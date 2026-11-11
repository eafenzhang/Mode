/* eslint-disable max-lines -- Host 生命周期（注册、Control 常驻、断链重连、token 轮换、RPC 代理）必须是同一状态所有者，拆分会让重连与登出竞态跨文件。 */
import { hostname } from "node:os";

import {
  ControlLink,
  DEFAULT_RECONNECT_CAP_MS,
  type DeviceDescriptor,
  type RemoteConnection,
  type RemoteSocket,
} from "@mode/remote-core";
import type { IRemoteV3Service } from "@mode/services";
import type {
  RemoteV3BeginLoginParams,
  RemoteV3ConfigureParams,
  RemoteV3ConfirmPairingParams,
  RemoteV3DeviceSummary,
  RemoteV3GithubChallenge,
  RemoteV3HostState,
  RemoteV3PairingClaim,
  RemoteV3PairingStart,
  RemoteV3PairingState,
  RemoteV3HttpRequestParams,
  RemoteV3HttpResponse,
  RemoteV3RemoteDesktopSession,
  RemoteV3RemoteDesktopStartParams,
  RemoteV3RequestParams,
  RemoteV3RequestResult,
  RemoteV3ResetPinParams,
  RemoteV3TunnelEvent,
  RemoteV3TunnelOpenParams,
} from "@mode/shared";

import {
  fetchRemoteV3Account,
  fetchRemoteV3Peer,
  isRemoteV3AuthError,
  normalizeRemoteServerUrl,
  refreshRemoteV3Tokens,
  registerRemoteV3Host,
  RemoteV3ApiError,
} from "./api.js";
import {
  beginRemoteV3Pairing,
  cancelRemoteV3Pairing,
  confirmRemoteV3Pairing,
  getRemoteV3Pairing,
} from "./apiPairing.js";
import {
  clearRemoteV3AccountName,
  clearRemoteV3Tokens,
  loadOrCreateRemoteV3Config,
  resetRemoteV3PeerPin,
  rotateRemoteV3ClientIdentity,
  rotateRemoteV3HostIdentity,
  saveRemoteV3Account,
  saveRemoteV3Server,
  saveRemoteV3ServerUrl,
  saveRemoteV3Tokens,
  verifyOrPinPeer,
  type RemoteV3StoredConfig,
  type RemoteV3Tokens,
} from "./identity.js";
import { RemoteV3ClientRuntime } from "./client.js";
import { connectWebSocket } from "./wsSocket.js";
import { attachHostConnection } from "./hostConnection.js";
import { RemoteV3GithubLogin } from "./login.js";
import { RemoteDesktopTunnelProxy } from "./rdProxy.js";
import { startV3RemoteDesktopSession } from "./remoteDesktop.js";
import { RemoteV3HostStateStore } from "./stateStore.js";
import { RemoteV3TunnelRuntime } from "./tunnels.js";

export interface RemoteV3HostServiceOptions {
  credentials: import("@mode/services").ICredentialService;
  env: NodeJS.ProcessEnv;
  log: (line: string) => void;
  deviceName?: string;
  platform?: string;
  version?: string;
  listWorkspaces?: () => Promise<Array<{ path: string; name?: string }>>;
  /** 本机 LAN 服务可达地址（host:port）；用于设备目录的直连候选。 */
  listLanAddresses?: () => Promise<string[]>;
  connectSocket?: (url: string) => Promise<RemoteSocket>;
  fetchImpl?: typeof fetch;
  /** 目标 Host 本机 LAN 服务地址与令牌；用于把 `/ws*` 隧道桥接到本机 HTTP/WS。 */
  getLocalApi?: () => Promise<{ baseUrl: string; token: string } | null>;
}

/** Desktop Host 的远程控制 v3 管理面：设备身份、Control 常驻、加密 RPC allowlist。 */
export class RemoteV3HostService implements IRemoteV3Service {
  private readonly options: RemoteV3HostServiceOptions;
  private readonly stateStore = new RemoteV3HostStateStore();
  private config: RemoteV3StoredConfig | null = null;
  private link: ControlLink | null = null;
  private reconnectTimer: NodeJS.Timeout | null = null;
  private reconnectAttempt = 0;
  private refreshInFlight: Promise<{ accessToken: string; refreshToken: string }> | null = null;
  private readonly githubLogin: RemoteV3GithubLogin;
  private readonly client: RemoteV3ClientRuntime;
  private readonly tunnels: RemoteV3TunnelRuntime;
  private readonly rdProxy = new RemoteDesktopTunnelProxy();

  constructor(options: RemoteV3HostServiceOptions) {
    this.options = options;
    this.client = new RemoteV3ClientRuntime({
      credentials: options.credentials,
      fetchImpl: options.fetchImpl,
      connectSocket: options.connectSocket ?? connectWebSocket,
      getServerUrl: () => this.config?.serverUrl ?? null,
      getHostAccessToken: () => this.config?.tokens?.accessToken ?? null,
      refreshHostTokens: () => this.refreshHostTokens(),
      log: options.log,
    });
    this.tunnels = new RemoteV3TunnelRuntime((hostDeviceId, service, metadata) =>
      this.client.openStream(hostDeviceId, service, metadata),
    );
    this.githubLogin = new RemoteV3GithubLogin({
      env: options.env,
      fetchImpl: options.fetchImpl,
      getServerUrl: () => this.config?.serverUrl ?? null,
      configure: ({ serverUrl, webSessionToken }) => this.configure({ serverUrl, webSessionToken }),
      onPendingChange: (pending) => this.stateStore.patch({ pendingLogin: pending }),
      log: options.log,
    });
  }

  async initialize(): Promise<void> {
    this.config = await loadOrCreateRemoteV3Config(this.options.credentials);
    this.applyConfigState();
    if (this.config.serverUrl && this.config.tokens) {
      this.stateStore.patch({ status: "connecting" });
      void this.startLink();
    }
  }

  async getState(): Promise<RemoteV3HostState> {
    return this.stateStore.snapshot;
  }

  async configure(params: RemoteV3ConfigureParams): Promise<RemoteV3HostState> {
    const serverUrl = normalizeRemoteServerUrl(params.serverUrl);
    const previous = this.requireConfig();
    this.client.dispose();
    const name =
      params.deviceName?.trim() ||
      this.config?.deviceName ||
      this.options.deviceName ||
      `Mode Host ${hostname()}`;

    const account = await fetchRemoteV3Account({
      serverUrl,
      webSessionToken: params.webSessionToken,
      fetchImpl: this.options.fetchImpl,
    });
    let config = previous;
    if (previous.accountId && previous.accountId !== account.id) {
      config = await this.rotateIdentityForAccountChange("account_changed");
    }
    await this.client.clearCredentials();

    let descriptor: DeviceDescriptor = {
      deviceId: config.deviceId,
      name,
      role: "host",
      platform: this.options.platform ?? "desktop",
      identityKey: config.identity.identityKey,
    };
    if (this.options.version) descriptor.clientVersion = this.options.version;
    const addresses = (await this.options.listLanAddresses?.().catch(() => [])) ?? [];
    let registration;
    try {
      registration = await registerRemoteV3Host({
        serverUrl,
        webSessionToken: params.webSessionToken,
        descriptor,
        ...(addresses.length ? { addresses: addresses.slice(0, 8) } : {}),
        fetchImpl: this.options.fetchImpl,
      });
    } catch (error) {
      // 409 表示该 deviceId 已不可用（属于其他账号，或被账号主撤销/删除）；
      // 轮换一次身份后重试，避免用户只能手动清凭据。第二次仍 409 则直接失败。
      if (!(error instanceof RemoteV3ApiError) || error.status !== 409) {
        throw error;
      }
      config = await this.rotateIdentityForAccountChange("device_conflict");
      descriptor = {
        deviceId: config.deviceId,
        name,
        role: "host",
        platform: this.options.platform ?? "desktop",
        identityKey: config.identity.identityKey,
      };
      if (this.options.version) descriptor.clientVersion = this.options.version;
      registration = await registerRemoteV3Host({
        serverUrl,
        webSessionToken: params.webSessionToken,
        descriptor,
        ...(addresses.length ? { addresses: addresses.slice(0, 8) } : {}),
        fetchImpl: this.options.fetchImpl,
      });
    }
    const tokens: RemoteV3Tokens = {
      accessToken: registration.accessToken,
      accessExpiresAt: registration.accessExpiresAt,
      refreshToken: registration.refreshToken,
      refreshExpiresAt: registration.refreshExpiresAt,
    };
    await saveRemoteV3Server(this.options.credentials, serverUrl, name, tokens);
    await saveRemoteV3Account(this.options.credentials, registration.accountId, account.displayName);
    this.config = {
      ...config,
      accountId: registration.accountId,
      accountName: account.displayName,
      serverUrl,
      deviceName: name,
      tokens: {
        accessToken: tokens.accessToken,
        accessExpiresAt: 0,
        refreshToken: tokens.refreshToken,
        refreshExpiresAt: 0,
      },
    };
    this.stateStore.patch({
      serverUrl,
      accountName: account.displayName,
      deviceId: config.deviceId,
      deviceName: name,
      fingerprint: config.fingerprint,
      error: null,
    });
    await this.startLink();
    return this.stateStore.snapshot;
  }

  private async rotateIdentityForAccountChange(
    reason: "account_changed" | "device_conflict",
  ): Promise<RemoteV3StoredConfig> {
    this.options.log(`remote_v3_identity_rotated reason=${reason}`);
    await rotateRemoteV3HostIdentity(this.options.credentials);
    await rotateRemoteV3ClientIdentity(this.options.credentials);
    const next = await loadOrCreateRemoteV3Config(this.options.credentials);
    this.config = next;
    this.client.reset();
    return next;
  }

  async beginGithubLogin(params: RemoteV3BeginLoginParams = {}): Promise<RemoteV3GithubChallenge> {
    const config = this.requireConfig();
    if (params.serverUrl?.trim()) {
      const serverUrl = normalizeRemoteServerUrl(params.serverUrl);
      await saveRemoteV3ServerUrl(this.options.credentials, serverUrl);
      this.config = { ...config, serverUrl };
      this.stateStore.patch({ serverUrl, error: null });
    }
    const challenge = await this.githubLogin.begin();
    this.stateStore.patch({
      status: "unconfigured",
      serverUrl:
        this.config?.serverUrl ?? this.options.env.MODE_REMOTE_V3_URL?.trim() ?? null,
      accountName: config.accountName,
      deviceId: config.deviceId,
      fingerprint: config.fingerprint,
      error: null,
    });
    return challenge;
  }

  async pollGithubLogin(): Promise<RemoteV3HostState> {
    await this.githubLogin.poll();
    return this.stateStore.snapshot;
  }

  cancelGithubLogin(): void {
    this.githubLogin.cancel();
  }

  async disconnectAccount(): Promise<RemoteV3HostState> {
    this.githubLogin.cancel();
    this.rdProxy.close();
    this.tunnels.closeAll();
    await this.client.clearCredentials();
    this.stopLink();
    await clearRemoteV3Tokens(this.options.credentials);
    await clearRemoteV3AccountName(this.options.credentials);
    if (this.config) {
      this.config.tokens = null;
      this.config.accountName = null;
    }
    this.stateStore.patch({
      status: "unconfigured",
      accountName: null,
      error: null,
      activeConnections: 0,
      pendingLogin: null,
    });
    return this.stateStore.snapshot;
  }

  async resetPeerPin(params: RemoteV3ResetPinParams): Promise<void> {
    await resetRemoteV3PeerPin(this.options.credentials, params.deviceId);
  }

  /** 本机 LAN 服务收到 `/ws/v1/direct` 首帧后调用；ticket 不匹配返回 false。 */
  acceptDirectSocket(connectionId: string, ticket: string, socket: RemoteSocket): boolean {
    return this.link?.acceptDirectSocket(connectionId, ticket, socket) ?? false;
  }

  async request(params: RemoteV3RequestParams): Promise<RemoteV3RequestResult> {
    return this.client.request(params);
  }

  async httpRequest(params: RemoteV3HttpRequestParams): Promise<RemoteV3HttpResponse> {
    return this.client.httpRequest(params);
  }

  async openTunnel(params: RemoteV3TunnelOpenParams): Promise<{ tunnelId: string }> {
    return this.tunnels.open(params);
  }

  async sendTunnel(tunnelId: string, data: string): Promise<void> {
    await this.tunnels.send(tunnelId, data);
  }

  async closeTunnel(tunnelId: string): Promise<void> {
    await this.tunnels.close(tunnelId);
  }

  readonly onTunnelEvent = (listener: (event: RemoteV3TunnelEvent) => void) => {
    return this.tunnels.onEvent(listener);
  };

  async listHosts(): Promise<RemoteV3DeviceSummary[]> {
    return this.client.listHosts();
  }

  async startRemoteDesktop(
    params: RemoteV3RemoteDesktopStartParams,
  ): Promise<RemoteV3RemoteDesktopSession> {
    return startV3RemoteDesktopSession(this.client, this.rdProxy, params);
  }

  async listDevices(): Promise<RemoteV3DeviceSummary[]> {
    return this.client.listDevicesForHost();
  }

  // ---- 跨账号配对：Host 角色（docs/specs/remote-control-v3-pairing.md）----

  async beginPairing(): Promise<RemoteV3PairingStart> {
    const config = this.requireConfig();
    if (!config.serverUrl || !config.tokens) throw new Error("Host 尚未登录远程控制 v3");
    return await beginRemoteV3Pairing({
      serverUrl: config.serverUrl,
      accessToken: config.tokens.accessToken,
      ...(this.options.fetchImpl ? { fetchImpl: this.options.fetchImpl } : {}),
    });
  }

  async confirmPairing(params: RemoteV3ConfirmPairingParams): Promise<RemoteV3HostState> {
    const config = this.requireConfig();
    if (!config.serverUrl || !config.tokens) throw new Error("Host 尚未登录远程控制 v3");
    const credentials = await confirmRemoteV3Pairing({
      serverUrl: config.serverUrl,
      accessToken: config.tokens.accessToken,
      pairingId: params.pairingId,
      code: params.code,
      ...(this.options.fetchImpl ? { fetchImpl: this.options.fetchImpl } : {}),
    });
    // 确认成功即轮换 Host 凭证：先落盘，再用新 token 重建 Control 链路。
    await saveRemoteV3Tokens(this.options.credentials, credentials);
    this.config = { ...config, tokens: credentials };
    await this.startLink();
    return this.stateStore.snapshot;
  }

  async claimPairing(ticket: string): Promise<RemoteV3PairingClaim> {
    return await this.client.claimPairing(ticket);
  }

  async redeemPairing(pairingId: string): Promise<RemoteV3PairingState> {
    return await this.client.redeemPairing(pairingId);
  }

  async getPairing(pairingId: string): Promise<RemoteV3PairingState> {
    const config = this.config;
    if (config?.serverUrl && config.tokens) {
      try {
        return await getRemoteV3Pairing({
          serverUrl: config.serverUrl,
          accessToken: config.tokens.accessToken,
          pairingId,
          ...(this.options.fetchImpl ? { fetchImpl: this.options.fetchImpl } : {}),
        });
      } catch (error) {
        // 本机不是该会话的 Host（跨账号由对方发起）：改用 Client 身份查询。
        if (!(error instanceof RemoteV3ApiError) || error.status !== 403) throw error;
      }
    }
    return await this.client.getPairing(pairingId);
  }

  async cancelPairing(pairingId: string): Promise<RemoteV3PairingState> {
    const config = this.config;
    if (config?.serverUrl && config.tokens) {
      try {
        return await cancelRemoteV3Pairing({
          serverUrl: config.serverUrl,
          accessToken: config.tokens.accessToken,
          pairingId,
          ...(this.options.fetchImpl ? { fetchImpl: this.options.fetchImpl } : {}),
        });
      } catch (error) {
        if (!(error instanceof RemoteV3ApiError) || error.status !== 403) throw error;
      }
    }
    return await this.client.cancelPairing(pairingId);
  }

  readonly onDidChange = (listener: (state: RemoteV3HostState) => void) =>
    this.stateStore.onDidChange(listener);

  private async startLink(): Promise<void> {
    this.clearReconnectTimer();
    this.stopLink();
    const config = this.requireConfig();
    const serverUrl = this.config?.serverUrl;
    const tokens = this.config?.tokens;
    if (!serverUrl || !tokens) {
      this.stateStore.patch({ status: "unconfigured" });
      return;
    }
    this.stateStore.patch({ status: "connecting", error: null });
    const link = this.createLink(serverUrl, tokens.accessToken);
    this.link = link;
    try {
      await link.start();
      if (this.link !== link) return;
      this.markOnline();
    } catch (initialError) {
      if (this.link !== link) return;
      await this.retryWithRefreshedTokens(serverUrl, config, tokens.refreshToken, initialError);
    }
  }

  /** access token 可能过期：先用 refresh token 换新凭证，再重连一次。 */
  private async retryWithRefreshedTokens(
    serverUrl: string,
    config: RemoteV3StoredConfig,
    refreshToken: string,
    initialError: unknown,
  ): Promise<void> {
    let refreshed: RemoteV3Tokens;
    try {
      refreshed = await refreshRemoteV3Tokens({
        serverUrl,
        deviceId: config.deviceId,
        refreshToken,
        fetchImpl: this.options.fetchImpl,
      });
    } catch (refreshError) {
      const fatal = isRemoteV3AuthError(initialError) || isRemoteV3AuthError(refreshError);
      this.failLink(refreshError, !fatal);
      return;
    }
    await saveRemoteV3Tokens(this.options.credentials, refreshed);
    this.config = {
      ...config,
      tokens: {
        accessToken: refreshed.accessToken,
        accessExpiresAt: 0,
        refreshToken: refreshed.refreshToken,
        refreshExpiresAt: 0,
      },
    };
    const retry = this.createLink(serverUrl, refreshed.accessToken);
    this.link = retry;
    try {
      await retry.start();
      if (this.link !== retry) return;
      this.markOnline();
    } catch (retryError) {
      if (this.link !== retry) return;
      this.failLink(retryError, !isRemoteV3AuthError(retryError));
    }
  }

  private markOnline(): void {
    this.reconnectAttempt = 0;
    this.stateStore.patch({ status: "online", error: null });
  }

  private failLink(error: unknown, retry: boolean): void {
    const message = error instanceof Error ? error.message : String(error);
    this.stateStore.patch({
      // 可恢复失败保持「重连中」；只有凭证失效才进入红色错误态。
      status: retry ? "reconnecting" : "error",
      error: message,
    });
    if (retry) {
      this.scheduleReconnect();
    } else {
      // 凭证被撤销/过期时继续重试只会掩盖真实原因；保持错误态等用户重新登录。
      this.options.log(`remote_v3_auth_failed error=${message}`);
    }
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer) return;
    if (!this.config?.serverUrl || !this.config.tokens) return;
    this.reconnectAttempt += 1;
    const base = Math.min(
      1000 * 2 ** Math.min(this.reconnectAttempt - 1, 5),
      DEFAULT_RECONNECT_CAP_MS,
    );
    // full jitter 下界 50%，避免多台设备在同一时刻一起重连。
    const delay = Math.max(500, Math.round(base * (0.5 + Math.random() * 0.5)));
    this.options.log(
      `remote_v3_reconnect_scheduled attempt=${this.reconnectAttempt} delay_ms=${delay}`,
    );
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      void this.startLink();
    }, delay);
    this.reconnectTimer.unref?.();
  }

  private clearReconnectTimer(): void {
    if (!this.reconnectTimer) return;
    clearTimeout(this.reconnectTimer);
    this.reconnectTimer = null;
  }

  private handleLinkClosed(link: ControlLink, details: { code: number; reason: string }): void {
    if (this.link !== link) return;
    this.link = null;
    this.options.log(
      `remote_v3_control_closed code=${details.code} reason=${details.reason || "<none>"}`,
    );
    this.stateStore.patch({
      status: "reconnecting",
      error: "Control 链路已断开，正在自动重连",
      activeConnections: 0,
    });
    this.scheduleReconnect();
  }

  /**
   * refresh token 是单次使用的：并发刷新会让服务端把第二次当成重放并撤销整个
   * token family。所有调用共享同一个在途 Promise。
   */
  private refreshHostTokens(): Promise<{ accessToken: string; refreshToken: string }> {
    if (this.refreshInFlight) return this.refreshInFlight;
    const inFlight = this.doRefreshHostTokens();
    this.refreshInFlight = inFlight;
    inFlight.then(
      () => {
        if (this.refreshInFlight === inFlight) this.refreshInFlight = null;
      },
      () => {
        if (this.refreshInFlight === inFlight) this.refreshInFlight = null;
      },
    );
    return inFlight;
  }

  private async doRefreshHostTokens(): Promise<{ accessToken: string; refreshToken: string }> {
    const config = this.requireConfig();
    const serverUrl = this.config?.serverUrl;
    const tokens = this.config?.tokens;
    if (!serverUrl || !tokens) throw new Error("Host 凭证缺失，无法刷新");
    const refreshed = await refreshRemoteV3Tokens({
      serverUrl,
      deviceId: config.deviceId,
      refreshToken: tokens.refreshToken,
      fetchImpl: this.options.fetchImpl,
    });
    await saveRemoteV3Tokens(this.options.credentials, refreshed);
    this.config = {
      ...config,
      tokens: {
        accessToken: refreshed.accessToken,
        accessExpiresAt: 0,
        refreshToken: refreshed.refreshToken,
        refreshExpiresAt: 0,
      },
    };
    return refreshed;
  }

  private createLink(serverUrl: string, accessToken: string): ControlLink {
    const config = this.requireConfig();
    const link = new ControlLink({
      role: "host",
      serverUrl,
      deviceId: config.deviceId,
      accessToken,
      identity: config.identity,
      clientVersion: this.options.version,
      connectSocket: this.options.connectSocket ?? connectWebSocket,
      fetchPeer: (deviceId) =>
        fetchRemoteV3Peer({
          serverUrl,
          accessToken: this.config?.tokens?.accessToken ?? accessToken,
          deviceId,
          fetchImpl: this.options.fetchImpl,
        }),
      verifyPeer: (peer) => verifyOrPinPeer(this.options.credentials, peer),
      onConnection: (connection) => this.attachConnection(connection),
      onClose: (details) => this.handleLinkClosed(link, details),
      onError: (error) => this.options.log(`remote_v3_error code=${error.code} error=${error.message}`),
    });
    return link;
  }

  private attachConnection(connection: RemoteConnection): void {
    const config = this.requireConfig();
    attachHostConnection(connection, {
      context: {
        deviceId: config.deviceId,
        name: this.config?.deviceName ?? hostname(),
        platform: this.options.platform ?? "desktop",
        ...(this.options.version ? { version: this.options.version } : {}),
        fingerprint: config.fingerprint,
        ...(this.options.listWorkspaces ? { listWorkspaces: this.options.listWorkspaces } : {}),
        ...(this.options.getLocalApi ? { getLocalApi: this.options.getLocalApi } : {}),
      },
      ...(this.options.getLocalApi ? { getLocalApi: this.options.getLocalApi } : {}),
      log: this.options.log,
      onOpen: () => this.stateStore.incrementConnections(),
      onClose: () => this.stateStore.decrementConnections(),
    });
  }

  private stopLink(): void {
    this.clearReconnectTimer();
    this.stateStore.patch({ activeConnections: 0 });
    this.link?.close();
    this.link = null;
  }

  private requireConfig(): RemoteV3StoredConfig {
    if (!this.config) throw new Error("远程控制 v3 配置尚未初始化");
    return this.config;
  }

  private applyConfigState(): void {
    const config = this.requireConfig();
    this.stateStore.setInitial({
      serverUrl: config.serverUrl,
      accountName: config.accountName,
      deviceId: config.deviceId,
      deviceName: config.deviceName,
      fingerprint: config.fingerprint,
    });
  }
}

export async function createRemoteV3HostService(
  options: RemoteV3HostServiceOptions,
): Promise<RemoteV3HostService> {
  const service = new RemoteV3HostService(options);
  await service.initialize();
  return service;
}
