import { randomUUID } from "node:crypto";

import {
  ControlLink,
  StreamMux,
  type BusinessFrame,
  type DirectAssignment,
  type RemoteConnection,
  type RemoteSocket,
  type RemoteStream,
} from "@mode/remote-core";
import type { ICredentialService } from "@mode/services";
import type {
  RemoteV3DeviceSummary,
  RemoteV3HttpRequestParams,
  RemoteV3HttpResponse,
  RemoteV3PairingClaim,
  RemoteV3PairingState,
  RemoteV3RequestParams,
  RemoteV3RequestResult,
} from "@mode/shared";

import {
  fetchRemoteV3Peer,
  listRemoteV3Devices,
  refreshRemoteV3Tokens,
  registerRemoteV3OwnedRole,
} from "./api.js";
import { createClientPairingMethods } from "./clientPairing.js";
import {
  clearRemoteV3ClientTokens,
  loadOrCreateRemoteV3ClientProfile,
  saveRemoteV3ClientTokens,
  verifyOrPinPeer,
  type RemoteV3ClientProfile,
} from "./identity.js";
import { createWeriftP2PAdapter } from "./p2pWerift.js";
import { connectDirectWebSocket } from "./wsSocket.js";

export interface RemoteV3ClientRuntimeDeps {
  credentials: ICredentialService;
  fetchImpl?: typeof fetch;
  connectSocket: (url: string) => Promise<RemoteSocket>;
  /** 测试注入点：LAN 直连适配器；缺省使用真实 ws。 */
  connectDirectSocket?: (
    assignment: DirectAssignment,
    connectionId: string,
    address: string,
    timeoutMs?: number,
  ) => Promise<RemoteSocket>;
  getServerUrl: () => string | null;
  getHostAccessToken: () => string | null;
  refreshHostTokens: () => Promise<{ accessToken: string; refreshToken: string }>;
  /** WebRTC P2P 适配器（werift）；提供后连接先协商 p2p，失败回落 Relay。 */
  p2pAdapter?: import("@mode/remote-core").P2PAdapter;
  onTransport?: (transport: import("@mode/remote-core").TransportKind) => void;
  log: (line: string) => void;
}

interface ClientConnection {
  link: ControlLink;
  connection: RemoteConnection;
  mux: StreamMux;
}

interface PendingRequest {
  hostDeviceId: string;
  resolve: (result: unknown) => void;
  reject: (error: Error) => void;
  timer: NodeJS.Timeout;
}

/** 本机作为 Client 角色：注册相反角色、缓存 E2EE 连接、执行一次 allowlist RPC。 */
export class RemoteV3ClientRuntime {
  private readonly deps: RemoteV3ClientRuntimeDeps;
  private readonly defaultP2pAdapter = createWeriftP2PAdapter();
  private profile: RemoteV3ClientProfile | null = null;
  private readonly connections = new Map<string, ClientConnection>();
  private readonly connecting = new Map<string, Promise<ClientConnection>>();
  private readonly pending = new Map<string, PendingRequest>();
  private clientRefreshInFlight: Promise<RemoteV3ClientProfile> | null = null;
  private generation = 0;

  private readonly pairing: ReturnType<typeof createClientPairingMethods>;

  constructor(deps: RemoteV3ClientRuntimeDeps) {
    this.deps = deps;
    this.pairing = createClientPairingMethods({
      credentials: deps.credentials,
      ...(deps.fetchImpl ? { fetchImpl: deps.fetchImpl } : {}),
      getServerUrl: () => deps.getServerUrl(),
      ensureProfile: (serverUrl) => this.ensureProfile(serverUrl),
      refreshClient: (serverUrl, profile) => this.refreshClient(serverUrl, profile),
      applyProfile: (profile) => {
        this.profile = profile;
      },
      dispose: () => this.dispose(),
    });
  }

  async request(params: RemoteV3RequestParams): Promise<RemoteV3RequestResult> {
    const { connection } = await this.getConnection(params.hostDeviceId);
    const id = randomUUID();
    const timeoutMs = params.timeoutMs ?? 15_000;
    const result = await new Promise<unknown>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error("远程 RPC 超时"));
      }, timeoutMs);
      this.pending.set(id, { hostDeviceId: params.hostDeviceId, resolve, reject, timer });
      connection.send({
        v: 1,
        type: "rpc.request",
        id,
        method: params.method,
        params: params.params ?? {},
      });
    });
    return { result };
  }

  async listDevicesForHost(): Promise<RemoteV3DeviceSummary[]> {
    const serverUrl = this.deps.getServerUrl();
    const accessToken = this.deps.getHostAccessToken();
    if (!serverUrl || !accessToken) throw new Error("Host 尚未登录远程控制 v3");
    try {
      return await listRemoteV3Devices({
        serverUrl,
        accessToken,
        fetchImpl: this.deps.fetchImpl,
      });
    } catch {
      const refreshed = await this.deps.refreshHostTokens();
      return await listRemoteV3Devices({
        serverUrl,
        accessToken: refreshed.accessToken,
        fetchImpl: this.deps.fetchImpl,
      });
    }
  }

  async listHosts(): Promise<RemoteV3DeviceSummary[]> {
    const serverUrl = this.deps.getServerUrl();
    if (!serverUrl) throw new Error("远程控制 v3 服务地址未配置");
    let profile = await this.ensureProfile(serverUrl);
    if (!profile.accessToken) throw new Error("Client access token 缺失");
    try {
      return await listRemoteV3Devices({
        serverUrl,
        accessToken: profile.accessToken,
        fetchImpl: this.deps.fetchImpl,
      });
    } catch {
      profile = await this.refreshClient(serverUrl, profile);
      if (!profile.accessToken) throw new Error("Client access token 缺失");
      return await listRemoteV3Devices({
        serverUrl,
        accessToken: profile.accessToken,
        fetchImpl: this.deps.fetchImpl,
      });
    }
  }

  async httpRequest(params: RemoteV3HttpRequestParams): Promise<RemoteV3HttpResponse> {
    const result = await this.request({
      hostDeviceId: params.hostDeviceId,
      method: "remote.http.request",
      params: {
        method: params.method,
        path: params.path,
        headers: params.headers ?? {},
        ...(params.bodyBase64 ? { bodyBase64: params.bodyBase64 } : {}),
      },
      ...(params.timeoutMs ? { timeoutMs: params.timeoutMs } : {}),
    });
    return result.result as RemoteV3HttpResponse;
  }

  async openStream(
    hostDeviceId: string,
    service: string,
    metadata: Record<string, unknown>,
  ): Promise<RemoteStream> {
    const { mux } = await this.getConnection(hostDeviceId);
    return mux.open(service, metadata);
  }

  dispose(): void {
    for (const entry of this.connections.values()) {
      entry.mux.dispose();
      entry.link.close();
    }
    this.connections.clear();
    this.connecting.clear();
    for (const request of this.pending.values()) {
      clearTimeout(request.timer);
      request.reject(new Error("Client runtime disposed"));
    }
    this.pending.clear();
  }

  /** 账号切换：丢弃内存中的身份与连接，下次请求从凭据服务重新加载。 */
  reset(): void {
    this.dispose();
    this.profile = null;
    this.generation += 1;
  }

  async clearCredentials(): Promise<void> {
    this.dispose();
    this.profile = null;
    this.generation += 1;
    await clearRemoteV3ClientTokens(this.deps.credentials);
  }

  private getConnection(hostDeviceId: string): Promise<ClientConnection> {
    const existing = this.connections.get(hostDeviceId);
    if (existing) return Promise.resolve(existing);
    const inFlight = this.connecting.get(hostDeviceId);
    if (inFlight) return inFlight;
    const promise = this.createConnection(hostDeviceId).finally(() => {
      this.connecting.delete(hostDeviceId);
    });
    this.connecting.set(hostDeviceId, promise);
    return promise;
  }

  private async createConnection(hostDeviceId: string): Promise<ClientConnection> {
    const serverUrl = this.deps.getServerUrl();
    if (!serverUrl) throw new Error("远程控制 v3 服务地址未配置");
    const profile = await this.ensureProfile(serverUrl);
    try {
      return await this.startClient(serverUrl, profile, hostDeviceId);
    } catch {
      const refreshed = await this.refreshClient(serverUrl, profile);
      return await this.startClient(serverUrl, refreshed, hostDeviceId);
    }
  }

  async claimPairing(ticket: string): Promise<RemoteV3PairingClaim> {
    return await this.pairing.claimPairing(ticket);
  }

  async getPairing(pairingId: string): Promise<RemoteV3PairingState> {
    return await this.pairing.getPairing(pairingId);
  }

  async cancelPairing(pairingId: string): Promise<RemoteV3PairingState> {
    return await this.pairing.cancelPairing(pairingId);
  }

  async redeemPairing(pairingId: string): Promise<RemoteV3PairingState> {
    return await this.pairing.redeemPairing(pairingId);
  }

  private async ensureProfile(serverUrl: string): Promise<RemoteV3ClientProfile> {
    if (!this.profile) {
      this.profile = await loadOrCreateRemoteV3ClientProfile(this.deps.credentials);
    }
    if (this.profile.accessToken) return this.profile;
    const hostAccessToken = this.deps.getHostAccessToken();
    if (!hostAccessToken) throw new Error("Host 尚未登录，无法注册 Client 角色");
    const base = this.profile;
    const identity = base.identity;
    const generation = this.generation;
    const describe = () => ({
      deviceId: base.deviceId,
      name: "Mode Client",
      role: "client" as const,
      platform: "desktop",
      identityKey: identity.identityKey,
    });
    let registration;
    try {
      registration = await registerRemoteV3OwnedRole({
        serverUrl,
        accessToken: hostAccessToken,
        descriptor: describe(),
        fetchImpl: this.deps.fetchImpl,
      });
    } catch {
      const refreshedHost = await this.deps.refreshHostTokens();
      registration = await registerRemoteV3OwnedRole({
        serverUrl,
        accessToken: refreshedHost.accessToken,
        descriptor: describe(),
        fetchImpl: this.deps.fetchImpl,
      });
    }
    if (generation !== this.generation) {
      throw new Error("Client 身份已切换，请重试");
    }
    await saveRemoteV3ClientTokens(this.deps.credentials, registration);
    this.profile = {
      ...base,
      accessToken: registration.accessToken,
      refreshToken: registration.refreshToken,
    };
    return this.profile;
  }

  /** refresh token 单次使用；并发刷新会被服务端判定为重放并撤销 token family。 */
  private refreshClient(
    serverUrl: string,
    profile: RemoteV3ClientProfile,
  ): Promise<RemoteV3ClientProfile> {
    if (this.clientRefreshInFlight) return this.clientRefreshInFlight;
    const inFlight = this.doRefreshClient(serverUrl, profile);
    this.clientRefreshInFlight = inFlight;
    inFlight.then(
      () => {
        if (this.clientRefreshInFlight === inFlight) this.clientRefreshInFlight = null;
      },
      () => {
        if (this.clientRefreshInFlight === inFlight) this.clientRefreshInFlight = null;
      },
    );
    return inFlight;
  }

  private async doRefreshClient(
    serverUrl: string,
    profile: RemoteV3ClientProfile,
  ): Promise<RemoteV3ClientProfile> {
    if (!profile.refreshToken) throw new Error("Client refresh token 缺失");
    const generation = this.generation;
    const tokens = await refreshRemoteV3Tokens({
      serverUrl,
      deviceId: profile.deviceId,
      refreshToken: profile.refreshToken,
      fetchImpl: this.deps.fetchImpl,
    });
    if (generation !== this.generation) {
      throw new Error("Client 身份已切换，忽略过期刷新结果");
    }
    await saveRemoteV3ClientTokens(this.deps.credentials, tokens);
    const next = { ...profile, accessToken: tokens.accessToken, refreshToken: tokens.refreshToken };
    this.profile = next;
    return next;
  }

  private async startClient(
    serverUrl: string,
    profile: RemoteV3ClientProfile,
    hostDeviceId: string,
  ): Promise<ClientConnection> {
    if (!profile.accessToken) throw new Error("Client access token 缺失");
    const accessToken = profile.accessToken;
    const link = new ControlLink({
      role: "client",
      serverUrl,
      deviceId: profile.deviceId,
      accessToken,
      identity: profile.identity,
      connectSocket: this.deps.connectSocket,
      p2p: { adapter: this.deps.p2pAdapter ?? this.defaultP2pAdapter, timeoutMs: 8000 },
      onTransport: (transport) => this.deps.onTransport?.(transport),
      onError: (error) => this.deps.log(`remote_v3_client_error code=${error.code}`),
      fetchPeer: (deviceId) =>
        fetchRemoteV3Peer({
          serverUrl,
          accessToken: this.profile?.accessToken ?? accessToken,
          deviceId,
          fetchImpl: this.deps.fetchImpl,
        }),
      verifyPeer: (peer) => verifyOrPinPeer(this.deps.credentials, peer),
      openDirectSocket: async (direct, connectionId) => {
        const connectDirect = this.deps.connectDirectSocket ?? connectDirectWebSocket;
        let lastError: unknown = null;
        // 直连只是快路径：给整组地址一个短预算，失败立即回落 Relay，
        // 不能让不可达的 LAN 地址把首连拖到 RPC 超时。
        const deadline = Date.now() + 2_000;
        for (const address of direct.addresses.slice(0, 4)) {
          const remaining = deadline - Date.now();
          if (remaining <= 0) break;
          try {
            return await connectDirect(direct, connectionId, address, Math.min(remaining, 1_500));
          } catch (error) {
            lastError = error;
          }
        }
        throw lastError instanceof Error ? lastError : new Error("没有可用的 LAN 直连地址");
      },
    });
    try {
      await link.start();
      const connection = await link.connect(hostDeviceId);
      const mux = new StreamMux({
        send: (frame) => connection.send(frame),
        onStream: () => undefined,
      });
      const entry: ClientConnection = { link, connection, mux };
      this.connections.set(hostDeviceId, entry);
      connection.onFrame((frame) => {
        if (!mux.handle(frame)) this.dispatch(frame);
      });
      connection.onClose(() => {
        mux.dispose();
        if (this.connections.get(hostDeviceId) === entry) this.connections.delete(hostDeviceId);
        // 断链后不要等 15s RPC 超时：同设备的在途请求立刻失败，调用方可以马上重试。
        this.rejectPendingForHost(hostDeviceId, new Error("远程连接已断开，请重试"));
      });
      return entry;
    } catch (error) {
      // Control 已握手但 connect 失败时不能把 link 留下：每次重试都会多一条常驻 WS。
      link.close();
      throw error;
    }
  }

  private rejectPendingForHost(hostDeviceId: string, error: Error): void {
    for (const [id, request] of this.pending) {
      if (request.hostDeviceId !== hostDeviceId) continue;
      clearTimeout(request.timer);
      this.pending.delete(id);
      request.reject(error);
    }
  }

  private dispatch(frame: BusinessFrame): void {
    if (frame.type !== "rpc.response" && frame.type !== "rpc.error") return;
    const request = this.pending.get(frame.id);
    if (!request) return;
    clearTimeout(request.timer);
    this.pending.delete(frame.id);
    if (frame.type === "rpc.response") {
      request.resolve(frame.result);
    } else {
      request.reject(new Error(`${frame.code}: ${frame.message}`));
    }
  }
}
