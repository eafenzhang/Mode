/**
 * Mode Cloud 个人账号与设备目录 Host 实现（docs/specs/cloud-personal-accounts.md，v2）。
 *
 * 身份方式：仅 GitHub Device Flow。发起登录即自动打开授权页并由 Host 自动轮询，
 * 授权成功后自动 enroll 本机（Ed25519 断言 + 上报本机访问令牌/地址）并启动隧道。
 * 连接 = connectDevice 从目录读令牌写 lan:peer:*，同账号即信任，无 6 位配对码。
 */
import { Emitter } from "@mode/rpc";
import {
  CLOUD_ACCOUNT_ID_CREDENTIAL_KEY,
  CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY,
  CLOUD_DEVICES_PATH,
  CLOUD_RELAY_TOKEN_CREDENTIAL_KEY,
  CLOUD_SESSION_TOKEN_CREDENTIAL_KEY,
  DEFAULT_CLOUD_CONTROL_BASE_URL,
  cloudDeviceSummarySchema,
  buildLanPeerMetaKey,
  buildLanPeerTokenKey,
  dedupeLanPeerEndpoints,
  parseLanPeerMeta,
  sortLanPeerEndpoints,
  type CloudAccountState,
  type CloudDeviceSummary,
  type LanPeerEndpoint,
} from "@mode/shared";
import { ICredentialService, ICloudAccountService, type ServiceCollection } from "@mode/services";

import { createCloudGithubLogin } from "./cloudGithubLogin.js";
import {
  loadCloudDeploymentConfig,
  type CloudDeploymentConfig,
} from "./cloudDeploymentConfig.js";
import { fetchWithTimeout } from "./lanPeerTransport.js";

const DIRECTORY_TIMEOUT_MS = 10_000;

export interface CloudAccessServerHandle {
  getState(): { port: number | null; addresses?: string[] };
  startRelayTunnel(token: string): void;
  /** 云账号同信任主控的固定令牌：enroll 时上报给目录（docs/specs/cloud-personal-accounts.md）。 */
  ensureCloudPeerToken(): Promise<string>;
}

export interface CreateCloudAccountHostServiceOptions {
  services: ServiceCollection;
  log: (line: string) => void;
  env?: NodeJS.ProcessEnv;
  /** 部署配置（安装时写入）；缺省从部署文件/env 读取。 */
  deploymentConfig?: CloudDeploymentConfig;
  /** 登录成功后启动中继隧道；缺省 = 无法启动（设备在目录里显示离线）。 */
  getLanAccessServer?: () => CloudAccessServerHandle | null;
  /** 打开系统浏览器（GitHub 授权页）；缺省按平台用系统默认方式。 */
  openExternal?: (url: string) => void;
  /** 登录自动轮询的基础间隔（毫秒）；测试可注入更短间隔。 */
  loginPollIntervalMs?: number;
}

export interface CloudAccountHostService extends ICloudAccountService {
  dispose(): Promise<void>;
}

function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function normalizeBaseUrl(value: string): string {
  const url = new URL(value);
  if (url.protocol !== "https:") {
    throw new Error("Mode Cloud 地址必须是 HTTPS");
  }
  return url.toString().replace(/\/+$/u, "");
}

function requireCredentials(services: ServiceCollection): ICredentialService {
  const credentials = services.getOptional(ICredentialService);
  if (!credentials) {
    throw new Error("凭据服务不可用，无法绑定 Mode Cloud");
  }
  return credentials;
}

/** 系统默认浏览器打开 URL（Host 无 Electron 依赖时的兜底实现）。 */
function openExternalDefault(url: string): void {
  void import("node:child_process").then(({ spawn }) => {
    const platform = process.platform;
    try {
      if (platform === "win32") {
        spawn("cmd.exe", ["/c", "start", "", url], { detached: true, stdio: "ignore" }).unref();
      } else if (platform === "darwin") {
        spawn("open", [url], { detached: true, stdio: "ignore" }).unref();
      } else {
        spawn("xdg-open", [url], { detached: true, stdio: "ignore" }).unref();
      }
    } catch {
      // 打不开浏览器不是登录失败：面板仍显示 user code 可手动输入。
    }
  });
}

/** "host:port"（IPv4 / [IPv6]:port）→ LAN 端点；无法解析返回 null。 */
function parseLanAddressEndpoint(address: string): LanPeerEndpoint | null {
  const trimmed = address.trim();
  const bracket = trimmed.match(/^\[(.+)\]:(\d{1,5})$/u);
  const plain = trimmed.match(/^([^:]+):(\d{1,5})$/u);
  const match = bracket ?? plain;
  if (!match) {
    return null;
  }
  const port = Number.parseInt(match[2]!, 10);
  if (!Number.isInteger(port) || port <= 0 || port > 65535) {
    return null;
  }
  return { scheme: "http", host: match[1]!, port, kind: "lan" };
}

export async function createCloudAccountHostService(
  options: CreateCloudAccountHostServiceOptions,
): Promise<CloudAccountHostService> {
  const { services, log } = options;
  const env = options.env ?? process.env;
  const credentials = requireCredentials(services);
  const deployment = options.deploymentConfig ?? loadCloudDeploymentConfig(env);
  let baseUrl = DEFAULT_CLOUD_CONTROL_BASE_URL;
  try {
    baseUrl = normalizeBaseUrl(
      env.MODE_CLOUD_BASE_URL?.trim() ||
        deployment.cloudBaseUrl ||
        DEFAULT_CLOUD_CONTROL_BASE_URL,
    );
  } catch (error) {
    log(`cloud_base_url_invalid ${describeError(error)}`);
  }
  // GitHub OAuth App 的 public client_id：显式 env 优先，其次部署配置。
  const githubClientId = (
    env.MODE_CLOUD_GITHUB_CLIENT_ID ||
    deployment.githubClientId ||
    ""
  ).trim();
  const openExternal = options.openExternal ?? openExternalDefault;

  const onDidChangeEmitter = new Emitter<CloudAccountState>();

  /** 绑定/登录成功后用云端下发的中继令牌启动隧道；handle 幂等。 */
  const ensureRelayTunnel = async (): Promise<void> => {
    const relayToken = (
      await credentials.load(CLOUD_RELAY_TOKEN_CREDENTIAL_KEY)
    )?.trim();
    const server = options.getLanAccessServer?.() ?? null;
    if (!relayToken || !server || !server.getState().port) {
      return;
    }
    server.startRelayTunnel(relayToken);
  };

  const readDirectory = async (
    token: string,
  ): Promise<{
    account: { id: string; displayName?: string };
    devices: CloudDeviceSummary[];
  }> => {
    const response = await fetchWithTimeout(
      `${baseUrl}${CLOUD_DEVICES_PATH}`,
      { headers: { authorization: `Bearer ${token}` } },
      DIRECTORY_TIMEOUT_MS,
    );
    if (!response.ok) {
      throw new Error(`云目录请求失败（HTTP ${response.status}）`);
    }
    let payload: unknown;
    try {
      payload = await response.json();
    } catch {
      throw new Error("云目录返回的不是合法 JSON");
    }
    const record = payload && typeof payload === "object" ? (payload as Record<string, unknown>) : {};
    const accountRaw =
      record.account && typeof record.account === "object"
        ? (record.account as Record<string, unknown>)
        : {};
    const accountId = typeof accountRaw.id === "string" ? accountRaw.id.trim() : "";
    if (!accountId) {
      throw new Error("云目录缺少账号信息");
    }
    const devices: CloudDeviceSummary[] = [];
    if (Array.isArray(record.devices)) {
      for (const item of record.devices) {
        const parsed = cloudDeviceSummarySchema.safeParse(item);
        if (parsed.success) {
          devices.push(parsed.data);
        }
      }
    }
    const displayName =
      typeof accountRaw.displayName === "string" && accountRaw.displayName.trim()
        ? accountRaw.displayName.trim()
        : undefined;
    return {
      account: {
        id: accountId,
        ...(displayName ? { displayName } : {}),
      },
      devices,
    };
  };

  const readState = async (): Promise<CloudAccountState> => {
    const token = (await credentials.load(CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY))?.trim();
    if (!token) {
      return { status: "unbound" };
    }
    try {
      const { account, devices } = await readDirectory(token);
      await ensureRelayTunnel().catch(() => undefined);
      return { status: "bound", account, devices };
    } catch (error) {
      return {
        status: "error",
        error: describeError(error),
      };
    }
  };

  const githubLogin = createCloudGithubLogin({
    credentials,
    baseUrl,
    githubClientId,
    ensureRelayTunnel,
    readState,
    onState: (state) => onDidChangeEmitter.fire(state),
    log,
    ...(options.loginPollIntervalMs ? { loginPollIntervalMs: options.loginPollIntervalMs } : {}),
  });

  /** Host 驱动的登录轮询：发起即开浏览器，授权后自动回到 bound（v2 交互）。 */
  let loginLoop: { stop(): void } | null = null;
  /** 最近一次自动轮询的失败原因；重新发起登录或成功后清除。 */
  let lastLoginError: string | undefined;

  const startLoginLoop = (getDeviceIdentity: () => Promise<{
    accessToken: string;
    addresses?: string[];
  }>): void => {
    loginLoop?.stop();
    let active = true;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const tick = async (): Promise<void> => {
      if (!active) {
        return;
      }
      try {
        const state = await githubLogin.poll(getDeviceIdentity);
        if (!active) {
          return;
        }
        if (state.status === "bound") {
          active = false;
          loginLoop = null;
          return;
        }
      } catch (error) {
        if (!active) {
          return;
        }
        // 硬错误（过期/拒绝/网络终止）：停止自动轮询，把原因带回 UI。
        active = false;
        loginLoop = null;
        lastLoginError = describeError(error);
        onDidChangeEmitter.fire({
          ...(await readState()),
          loginError: lastLoginError,
        });
        return;
      }
      if (active) {
        timer = setTimeout(() => void tick(), githubLogin.currentIntervalMs());
      }
    };
    timer = setTimeout(() => void tick(), githubLogin.currentIntervalMs());
    loginLoop = {
      stop() {
        active = false;
        if (timer) {
          clearTimeout(timer);
          timer = null;
        }
        githubLogin.cancel();
      },
    };
  };

  const selfDeviceIdentity = async (): Promise<{
    accessToken: string;
    addresses?: string[];
  }> => {
    const server = options.getLanAccessServer?.() ?? null;
    if (!server) {
      throw new Error("局域网访问服务未启动，无法上报本机访问令牌");
    }
    const accessToken = await server.ensureCloudPeerToken();
    return {
      accessToken,
      addresses: server.getState().addresses ?? [],
    };
  };

  const listDevices = async (): Promise<CloudDeviceSummary[]> => {
    const token = (await credentials.load(CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY))?.trim();
    if (!token) {
      throw new Error("尚未登录 Mode Cloud");
    }
    const { devices } = await readDirectory(token);
    return devices;
  };

  /** 目录直连（docs/specs/cloud-personal-accounts.md，v2）：无配对码，同账号即信任。 */
  const connectDevice = async (
    deviceId: string,
  ): Promise<{ serverId: string; name: string }> => {
    const devices = await listDevices();
    const device = devices.find((item) => item.id === deviceId);
    if (!device) {
      throw new Error("云目录中找不到该设备，请刷新后重试");
    }
    if (!device.accessToken) {
      throw new Error("该设备还没有可用的访问令牌：请在对端重新登录 GitHub");
    }
    const url = new URL(device.relayUrl);
    if (url.protocol !== "https:") {
      throw new Error("云设备地址必须是 HTTPS");
    }
    const path = url.pathname.replace(/\/+$/u, "");
    const port = url.port ? Number.parseInt(url.port, 10) : 443;
    const endpoints: LanPeerEndpoint[] = [
      { scheme: "https", host: url.hostname, port, kind: "public", ...(path ? { path } : {}) },
      ...(device.addresses ?? [])
        .map((address) => parseLanAddressEndpoint(address))
        .filter((endpoint): endpoint is LanPeerEndpoint => endpoint !== null),
    ];
    await credentials.save(buildLanPeerTokenKey(device.id), device.accessToken);
    // 重复连接同一设备时合并端点：本次端点优先，既有 LAN 端点保留在后。
    const existing = parseLanPeerMeta(await credentials.load(buildLanPeerMetaKey(device.id)));
    const existingLan = (existing?.endpoints ?? []).filter(
      (endpoint) => endpoint.kind === "lan",
    );
    const merged = sortLanPeerEndpoints(
      dedupeLanPeerEndpoints([...endpoints, ...existingLan]),
    );
    await credentials.save(
      buildLanPeerMetaKey(device.id),
      JSON.stringify({ endpoints: merged, name: device.name, pairedAt: Date.now() }),
    );
    log(`cloud_device_connect device=${device.id.slice(0, 8)}…`);
    return { serverId: device.id, name: device.name };
  };

  const service: CloudAccountHostService = {
    getState: async () => {
      const state = await readState();
      return lastLoginError && state.status === "unbound"
        ? { ...state, loginError: lastLoginError }
        : state;
    },
    beginGithubLogin: async () => {
      loginLoop?.stop();
      loginLoop = null;
      lastLoginError = undefined;
      const challenge = await githubLogin.begin();
      // v2 交互：发起登录即打开授权页，Host 自动轮询，授权后自动回到已登录。
      try {
        openExternal(challenge.verificationUri);
      } catch {
        // 打不开浏览器不是失败：面板仍显示 user code 可手动输入。
      }
      startLoginLoop(selfDeviceIdentity);
      return challenge;
    },
    pollGithubLogin: async () => {
      // 兼容入口：自动轮询由 Host 驱动；这里做一次手动步进。
      return await githubLogin.poll(selfDeviceIdentity);
    },
    cancelGithubLogin: () => {
      loginLoop?.stop();
      loginLoop = null;
      lastLoginError = undefined;
    },
    async unbind() {
      loginLoop?.stop();
      loginLoop = null;
      // 先撤销云端会话（best-effort），再清本地凭据；设备撤销走目录里的显式操作。
      try {
        await githubLogin.logout();
      } catch {
        // 云端不可达时也要完成本地清理；服务端会话按 TTL 过期兜底。
      }
      await credentials.delete(CLOUD_ACCOUNT_ID_CREDENTIAL_KEY);
      await credentials.delete(CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY);
      await credentials.delete(CLOUD_RELAY_TOKEN_CREDENTIAL_KEY);
      await credentials.delete(CLOUD_SESSION_TOKEN_CREDENTIAL_KEY);
      const state = await readState();
      onDidChangeEmitter.fire(state);
      return state;
    },
    listDevices,
    connectDevice,
    onDidChange: onDidChangeEmitter.event,
    async dispose() {
      loginLoop?.stop();
      loginLoop = null;
      onDidChangeEmitter.dispose();
    },
  };
  return service;
}
