// 浏览器路径必须走 web 入口：node 入口会引入 Buffer/crypto，生产包里直接 ReferenceError。
import { decodeBase64Url, encodeBase64Url } from "@mode/remote-core/web";
import {
  WebAccountClient,
  WebControlLink,
  importWebIdentity,
  type WebBusinessStream,
  type WebCredentialStore,
  type WebDeviceProfile,
  type WebDeviceSummary,
  type WebGithubChallenge,
  type WebRemoteConnection,
} from "@mode/remote-core/web";
import { Emitter } from "@mode/rpc";

import { createWebPairingMethods } from "./webPairing.js";
import { createWebRequestMethods } from "./webRequests.js";
import type { IRemoteV3Service } from "@mode/services";
import type {
  RemoteV3BeginLoginParams,
  RemoteV3ConfigureParams,
  RemoteV3DeviceSummary,
  RemoteV3HostState,
  RemoteV3RemoteDesktopSession,
  RemoteV3RemoteDesktopStartParams,
  RemoteV3ResetPinParams,
  RemoteV3TunnelEvent,
  RemoteV3TunnelOpenParams,
} from "@mode/shared";

/**
 * 浏览器 Client 角色的 `IRemoteV3Service`。
 *
 * 与桌面 Host 版的分工：账号会话存在浏览器（IndexedDB），Control/Noise 连接由本服务
 * 按 Host 缓存；Host 管理类操作（本机被控状态、peer pin）在浏览器没有对应事实源，
 * 明确拒绝而不是伪造成功。远程桌面在 Web 走 `/v3` 入口（见 remote-control-v3-web.md）。
 */
export interface WebRemoteV3ServiceOptions {
  store: WebCredentialStore;
  /** 读取已保存的账号会话（不发起网络请求）。 */
  loadProfile: () => Promise<WebDeviceProfile | null>;
  /** 测试注入点。 */
  createAccountClient?: (profile: WebDeviceProfile) => WebAccountClient;
  /** 登录/注册用（此时还没有 profile）；测试注入。 */
  createLoginClient?: (serverUrl: string, deviceName: string) => WebAccountClient;
  createLink?: (profile: WebDeviceProfile) => WebControlLink;
}

interface PendingLogin {
  client: WebAccountClient;
  deviceCode: string;
  interval: number;
}

interface HostAttachment {
  link: WebControlLink;
  connection: WebRemoteConnection;
}

const INITIAL_STATE: RemoteV3HostState = {
  status: "connecting",
  serverUrl: null,
  accountName: null,
  deviceId: null,
  deviceName: null,
  fingerprint: null,
  error: null,
  activeConnections: 0,
  pendingLogin: null,
};

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function fingerprintOf(profile: WebDeviceProfile): string | null {
  try {
    return importWebIdentity(profile.privateKey).identityKey.slice(0, 8);
  } catch {
    return null;
  }
}

function toSummary(device: WebDeviceSummary): RemoteV3DeviceSummary {
  return {
    deviceId: device.deviceId,
    name: device.name,
    role: device.role,
    platform: device.platform,
    identityKey: device.identityKey,
    online: device.online,
    addresses: device.addresses,
  };
}

export function createWebRemoteV3Service(options: WebRemoteV3ServiceOptions): IRemoteV3Service {
  const stateEmitter = new Emitter<RemoteV3HostState>();
  const tunnelEmitter = new Emitter<RemoteV3TunnelEvent>();
  let state: RemoteV3HostState = INITIAL_STATE;
  let profile: WebDeviceProfile | null = null;
  let loading: Promise<void> | null = null;
  let pending: PendingLogin | null = null;
  let pollTimer: ReturnType<typeof setTimeout> | null = null;
  const links = new Map<string, HostAttachment>();
  const tunnels = new Map<string, WebBusinessStream>();

  const setState = (patch: Partial<RemoteV3HostState>): void => {
    state = { ...state, ...patch };
    stateEmitter.fire(state);
  };

  const updateActive = (): void => setState({ activeConnections: links.size });

  const accountFor = (current: WebDeviceProfile): WebAccountClient =>
    options.createAccountClient?.(current) ??
    new WebAccountClient({
      store: options.store,
      serverUrl: current.serverUrl,
      deviceName: current.deviceName,
    });

  const linkFor = (current: WebDeviceProfile): WebControlLink =>
    options.createLink?.(current) ??
    new WebControlLink({
      serverUrl: current.serverUrl,
      deviceId: current.deviceId,
      accessToken: current.accessToken ?? "",
      identity: importWebIdentity(current.privateKey),
    });

  const ensureProfile = (): Promise<void> => {
    loading ??= (async () => {
      try {
        profile = await options.loadProfile();
        if (!profile) {
          setState({ ...INITIAL_STATE, status: "unconfigured" });
          return;
        }
        setState({
          status: profile.accessToken ? "online" : "error",
          serverUrl: profile.serverUrl,
          deviceId: profile.deviceId,
          deviceName: profile.deviceName,
          fingerprint: fingerprintOf(profile),
          error: profile.accessToken ? null : "登录已失效",
        });
      } catch (error) {
        setState({ ...INITIAL_STATE, status: "error", error: messageOf(error) });
      }
    })();
    return loading;
  };

  const requireProfile = async (): Promise<WebDeviceProfile> => {
    await ensureProfile();
    if (!profile?.accessToken) throw new Error("v3_login_required");
    return profile;
  };

  const listDevices = async (role: "host" | "client" | null): Promise<WebDeviceSummary[]> => {
    await ensureProfile();
    if (!profile?.accessToken) return [];
    const devices = await accountFor(profile).listHosts();
    return role ? devices.filter((device) => device.role === role) : devices;
  };

  const clearPoll = (): void => {
    if (pollTimer) clearTimeout(pollTimer);
    pollTimer = null;
  };

  const pollOnce = async (): Promise<void> => {
    const current = pending;
    if (!current) return;
    let result: Awaited<ReturnType<WebAccountClient["pollGithubDeviceLogin"]>>;
    try {
      result = await current.client.pollGithubDeviceLogin(current.deviceCode);
    } catch (error) {
      // 瞬时网络失败不清掉 pending：保持「等待授权」并按原间隔重试，避免用户重开 Device Flow。
      setState({ error: messageOf(error) });
      clearPoll();
      pollTimer = setTimeout(() => {
        pollTimer = null;
        void pollOnce();
      }, current.interval * 1_000);
      return;
    }
    if (result.status === "ok") {
      clearPoll();
      pending = null;
      profile = result.profile;
      setState({
        status: "online",
        serverUrl: result.profile.serverUrl,
        deviceId: result.profile.deviceId,
        deviceName: result.profile.deviceName,
        fingerprint: fingerprintOf(result.profile),
        error: null,
        pendingLogin: null,
      });
      return;
    }
    if (result.status === "expired" || result.status === "denied") {
      clearPoll();
      pending = null;
      setState({
        status: "error",
        error: result.status === "expired" ? "授权码已过期" : "授权被拒绝",
        pendingLogin: null,
      });
      return;
    }
    clearPoll();
    pollTimer = setTimeout(() => {
      pollTimer = null;
      void pollOnce();
    }, current.interval * 1_000);
  };

  const closeAllLinks = (): void => {
    for (const attachment of links.values()) {
      attachment.connection.close();
      attachment.link.close();
    }
    links.clear();
    for (const stream of tunnels.values()) stream.close("service disposed");
    tunnels.clear();
    updateActive();
  };

  const ensureConnection = async (hostDeviceId: string): Promise<HostAttachment> => {
    const existing = links.get(hostDeviceId);
    if (existing) return existing;
    const current = await requireProfile();
    const link = linkFor(current);
    await link.start();
    const connection = await link.connect(hostDeviceId);
    const attachment: HostAttachment = { link, connection };
    links.set(hostDeviceId, attachment);
    connection.onClose(() => {
      if (links.get(hostDeviceId) !== attachment) return;
      links.delete(hostDeviceId);
      updateActive();
    });
    updateActive();
    return attachment;
  };

  return {
    getState: async () => {
      await ensureProfile();
      return state;
    },
    configure: async (params: RemoteV3ConfigureParams) => {
      const deviceName = params.deviceName ?? "Mode Web";
      const client =
        options.createLoginClient?.(params.serverUrl, deviceName) ??
        new WebAccountClient({ store: options.store, serverUrl: params.serverUrl, deviceName });
      profile = await client.loginWithSessionToken(params.webSessionToken);
      loading = null;
      setState({
        status: "online",
        serverUrl: profile.serverUrl,
        deviceId: profile.deviceId,
        deviceName: profile.deviceName,
        fingerprint: fingerprintOf(profile),
        error: null,
      });
      return state;
    },
    beginGithubLogin: async (params?: RemoteV3BeginLoginParams) => {
      await ensureProfile();
      const serverUrl = (params?.serverUrl ?? state.serverUrl ?? "").trim();
      if (!serverUrl) throw new Error("请先填写账号服务地址");
      const deviceName = "Mode Web";
      const client =
        options.createLoginClient?.(serverUrl, deviceName) ??
        new WebAccountClient({ store: options.store, serverUrl, deviceName });
      const challenge: WebGithubChallenge = await client.beginGithubDeviceLogin();
      pending = {
        client,
        deviceCode: challenge.deviceCode,
        interval: challenge.interval,
      };
      setState({
        status: "connecting",
        serverUrl,
        error: null,
        pendingLogin: {
          userCode: challenge.userCode,
          verificationUri: challenge.verificationUri,
          expiresAt: Date.now() + challenge.expiresIn * 1_000,
        },
      });
      clearPoll();
      pollTimer = setTimeout(() => {
        pollTimer = null;
        void pollOnce();
      }, challenge.interval * 1_000);
      return {
        userCode: challenge.userCode,
        verificationUri: challenge.verificationUri,
        expiresIn: challenge.expiresIn,
        interval: challenge.interval,
      };
    },
    listDevices: async () => (await listDevices(null)).map(toSummary),
    listHosts: async () => (await listDevices("host")).map(toSummary),
    startRemoteDesktop: async (
      _params: RemoteV3RemoteDesktopStartParams,
    ): Promise<RemoteV3RemoteDesktopSession> => {
      throw new Error("浏览器端远程桌面请从 /v3 页面进入");
    },
    pollGithubLogin: async () => {
      await pollOnce();
      return state;
    },
    cancelGithubLogin: () => {
      clearPoll();
      pending = null;
      setState({
        status: profile ? "online" : "unconfigured",
        pendingLogin: null,
      });
    },
    disconnectAccount: async () => {
      await ensureProfile();
      const current = profile;
      profile = null;
      loading = null;
      if (current) {
        try {
          await accountFor(current).logout();
        } catch {
          // 本地退出优先：远端注销失败不阻断清理。
        }
      }
      closeAllLinks();
      setState({ ...INITIAL_STATE, status: "unconfigured" });
      return state;
    },
    ...createWebPairingMethods({
      requireProfile,
      accountFor,
      reloadProfile: () => options.loadProfile(),
      applyProfile: (next) => {
        profile = next;
        setState({
          status: "online",
          serverUrl: next.serverUrl,
          deviceId: next.deviceId,
          deviceName: next.deviceName,
          fingerprint: fingerprintOf(next),
          error: null,
        });
      },
    }),
    resetPeerPin: async (_params: RemoteV3ResetPinParams) => {
      throw new Error("浏览器端不保存 peer pin");
    },
    ...createWebRequestMethods(ensureConnection),
    openTunnel: async (params: RemoteV3TunnelOpenParams) => {
      const attachment = await ensureConnection(params.hostDeviceId);
      const stream = attachment.link.openStream("remote.ws", {
        path: params.path,
        ...(params.headers ? { headers: params.headers } : {}),
      });
      const tunnelId = crypto.randomUUID();
      tunnels.set(tunnelId, stream);
      stream.onData((bytes) => {
        tunnelEmitter.fire({ tunnelId, kind: "message", data: encodeBase64Url(bytes) });
      });
      stream.onClose((reason) => {
        tunnels.delete(tunnelId);
        tunnelEmitter.fire({ tunnelId, kind: "close", reason });
      });
      tunnelEmitter.fire({ tunnelId, kind: "open" });
      return { tunnelId };
    },
    sendTunnel: async (tunnelId: string, data: string) => {
      const stream = tunnels.get(tunnelId);
      if (!stream) throw new Error("隧道不存在或已关闭");
      stream.send(decodeBase64Url(data, "tunnel.data"));
    },
    closeTunnel: async (tunnelId: string) => {
      const stream = tunnels.get(tunnelId);
      if (!stream) return;
      tunnels.delete(tunnelId);
      stream.close("closed by client");
    },
    onTunnelEvent: tunnelEmitter.event,
    onDidChange: stateEmitter.event,
  };
}
