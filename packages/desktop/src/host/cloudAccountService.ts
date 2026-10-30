/**
 * Mode Cloud 个人账号与设备目录 Host 实现（docs/specs/cloud-personal-accounts.md）。
 *
 * 身份方式：GitHub Device Flow + 用户名密码；登录成功后用会话 enroll 本机设备
 * （Ed25519 断言），并保存每设备中继令牌启动隧道。配对仍复用 `pairLanPeer`
 * （6 位码 + 现有 /api/lan/pair），云端不接触 LAN 令牌。
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
  isLanAccessPairCodeFormat,
  type CloudAccountState,
  type CloudDeviceSummary,
  type LanAccessPairResult,
} from "@mode/shared";
import { ICredentialService, ICloudAccountService, type ServiceCollection } from "@mode/services";

import { createCloudGithubLogin } from "./cloudGithubLogin.js";
import { createCloudPasswordAuth } from "./cloudPasswordLogin.js";
import {
  loadCloudDeploymentConfig,
  type CloudDeploymentConfig,
} from "./cloudDeploymentConfig.js";
import { fetchWithTimeout } from "./lanPeerTransport.js";
import { pairLanPeer } from "./lanRemoteAttach.js";

const DIRECTORY_TIMEOUT_MS = 10_000;

export interface CreateCloudAccountHostServiceOptions {
  services: ServiceCollection;
  log: (line: string) => void;
  env?: NodeJS.ProcessEnv;
  /** 部署配置（安装时写入）；缺省从部署文件/env 读取。 */
  deploymentConfig?: CloudDeploymentConfig;
  /** 登录成功后启动中继隧道；缺省 = 无法启动（设备在目录里显示离线）。 */
  getLanAccessServer?: () => {
    getState(): { port: number | null };
    startRelayTunnel(token: string): void;
  } | null;
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
    account: { id: string; username?: string; displayName?: string };
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
    const username =
      typeof accountRaw.username === "string" && accountRaw.username.trim()
        ? accountRaw.username.trim()
        : undefined;
    const displayName =
      typeof accountRaw.displayName === "string" && accountRaw.displayName.trim()
        ? accountRaw.displayName.trim()
        : undefined;
    return {
      account: {
        id: accountId,
        ...(username ? { username } : {}),
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
  });
  const passwordAuth = createCloudPasswordAuth({
    credentials,
    baseUrl,
    ensureRelayTunnel,
    readState,
    onState: (state) => onDidChangeEmitter.fire(state),
    log,
  });

  const listDevices = async (): Promise<CloudDeviceSummary[]> => {
    const token = (await credentials.load(CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY))?.trim();
    if (!token) {
      throw new Error("尚未登录 Mode Cloud");
    }
    const { devices } = await readDirectory(token);
    return devices;
  };

  const pairDevice = async (params: {
    deviceId: string;
    code: string;
  }): Promise<LanAccessPairResult> => {
    const code = params.code.trim().toUpperCase();
    if (!isLanAccessPairCodeFormat(code)) {
      throw new Error("请输入被控端显示的 6 位配对码");
    }
    const devices = await listDevices();
    const device = devices.find((item) => item.id === params.deviceId);
    if (!device) {
      throw new Error("云目录中找不到该设备，请刷新后重试");
    }
    const url = new URL(device.relayUrl);
    if (url.protocol !== "https:") {
      throw new Error("云设备地址必须是 HTTPS");
    }
    const path = url.pathname.replace(/\/+$/u, "");
    const port = url.port ? Number.parseInt(url.port, 10) : 443;
    return pairLanPeer({
      credentials,
      host: url.hostname,
      port,
      code,
      scheme: "https",
      ...(path ? { path } : {}),
    });
  };

  const service: CloudAccountHostService = {
    getState: readState,
    beginGithubLogin: () => githubLogin.begin(),
    pollGithubLogin: () => githubLogin.poll(),
    cancelGithubLogin: () => githubLogin.cancel(),
    registerWithPassword: (params) => passwordAuth.register(params),
    loginWithPassword: (params) => passwordAuth.login(params),
    setAccountCredentials: (params) => passwordAuth.setCredentials(params),
    async unbind() {
      // 先撤销云端会话（best-effort），再清本地凭据；设备撤销走目录里的显式操作。
      try {
        await passwordAuth.logout();
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
    pairDevice,
    onDidChange: onDidChangeEmitter.event,
    async dispose() {
      onDidChangeEmitter.dispose();
    },
  };
  return service;
}
