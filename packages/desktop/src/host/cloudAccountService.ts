/**
 * Mode Cloud 账号与设备目录 Host 实现（docs/specs/mode-cloud-account-directory.md）。
 *
 * 账号锚点来自本机已绑定的微信 Bot；设备密钥与云凭据存加密凭据服务。
 * 配对仍复用 `pairLanPeer`（6 位码 + 现有 /api/lan/pair），云端不接触 LAN 令牌。
 */
import { hostname } from "node:os";

import { Emitter } from "@mode/rpc";
import {
  CLOUD_ACCOUNT_ID_CREDENTIAL_KEY,
  CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY,
  CLOUD_DEVICES_PATH,
  CLOUD_ENROLL_PATH,
  CLOUD_RELAY_TOKEN_CREDENTIAL_KEY,
  DEFAULT_CLOUD_CONTROL_BASE_URL,
  MODE_VERSION,
  cloudDeviceSummarySchema,
  isLanAccessPairCodeFormat,
  type BotConfig,
  type CloudAccountState,
  type CloudDeviceSummary,
  type LanAccessPairResult,
} from "@mode/shared";
import {
  IBotsService,
  ICloudAccountService,
  ICredentialService,
  type ServiceCollection,
} from "@mode/services";

import {
  canonicalJson,
  createEnrollmentNonce,
  loadOrCreateCloudDeviceIdentity,
  signCloudEnrollment,
} from "./cloudDeviceIdentity.js";
import {
  loadCloudDeploymentConfig,
  type CloudDeploymentConfig,
} from "./cloudDeploymentConfig.js";
import { startCloudAutoProvision } from "./cloudAutoProvision.js";
import { createCloudBotSync } from "./cloudBotSync.js";
import { createCloudWechatLogin } from "./cloudWechatLogin.js";
import { fetchWithTimeout } from "./lanPeerTransport.js";
import { pairLanPeer } from "./lanRemoteAttach.js";

const ENROLL_TIMEOUT_MS = 15_000;
const DIRECTORY_TIMEOUT_MS = 10_000;

class CloudApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
  }
}

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
  // 显式 env 优先，便于测试/临时覆盖；未设置时才用部署文件（零配置路径）。
  const enrollmentToken = (
    env.MODE_CLOUD_ENROLL_TOKEN ||
    deployment.enrollToken ||
    ""
  ).trim();
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

  const readBoundBot = async (): Promise<BotConfig | null> => {
    const bots = services.getOptional(IBotsService);
    if (!bots) {
      return null;
    }
    try {
      const list = await bots.listBots();
      return (
        list.find(
          (bot) => bot.enabled && bot.provider === "weixin" && Boolean(bot.providerUserId?.trim()),
        ) ?? null
      );
    } catch {
      return null;
    }
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
      throw new CloudApiError(`云目录请求失败（HTTP ${response.status}）`, response.status);
    }
    let payload: unknown;
    try {
      payload = await response.json();
    } catch {
      throw new CloudApiError("云目录返回的不是合法 JSON", 0);
    }
    const record = payload && typeof payload === "object" ? (payload as Record<string, unknown>) : {};
    const accountRaw =
      record.account && typeof record.account === "object"
        ? (record.account as Record<string, unknown>)
        : {};
    const accountId = typeof accountRaw.id === "string" ? accountRaw.id.trim() : "";
    if (!accountId) {
      throw new CloudApiError("云目录缺少账号信息", 0);
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
    return { account: { id: accountId, ...(displayName ? { displayName } : {}) }, devices };
  };

  const readState = async (): Promise<CloudAccountState> => {
    const bot = await readBoundBot();
    const boundWeixinName =
      bot?.displayName?.trim() || bot?.providerUserId?.trim() || undefined;
    const token = (await credentials.load(CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY))?.trim();
    const enrollmentConfigured = Boolean(enrollmentToken);
    const base = {
      enrollmentConfigured,
      ...(boundWeixinName ? { boundWeixinName } : {}),
    };
    if (!token) {
      return { ...base, status: enrollmentConfigured ? "unbound" : "unavailable" };
    }
    try {
      const { account, devices } = await readDirectory(token);
      await ensureRelayTunnel().catch(() => undefined);
      return { ...base, status: "bound", account, devices };
    } catch (error) {
      const unauthorized = error instanceof CloudApiError && error.status === 401;
      return {
        ...base,
        status: "error",
        error: unauthorized ? "云凭据已失效，请重新绑定" : describeError(error),
      };
    }
  };

  const wechatLogin = createCloudWechatLogin({
    credentials,
    baseUrl,
    enrollmentConfigured: Boolean(enrollmentToken),
    readBoundBot,
    ensureRelayTunnel,
    readState,
    onState: (state) => onDidChangeEmitter.fire(state),
    log,
  });
  const cloudBotSync = createCloudBotSync({
    services,
    credentials,
    baseUrl,
    enrollmentToken,
    readState,
    onState: (state) => onDidChangeEmitter.fire(state),
    log,
  });

  const bind = async (): Promise<CloudAccountState> => {
    if (!enrollmentToken) {
      throw new Error("未配置 MODE_CLOUD_ENROLL_TOKEN，无法绑定 Mode Cloud");
    }
    const bot = await readBoundBot();
    const providerUserId = bot?.providerUserId?.trim();
    if (!bot || !providerUserId) {
      throw new Error("请先在 Bot 设置里绑定微信账号");
    }
    const identity = await loadOrCreateCloudDeviceIdentity(credentials);
    const issuedAt = Date.now();
    const nonce = createEnrollmentNonce();
    const payload = canonicalJson({
      account: { provider: "weixin", providerUserId },
      device: { id: identity.deviceId, relayTunnelId: identity.relayTunnelId },
      nonce,
      issuedAt,
    });
    const displayName = bot.displayName?.trim();
    const response = await fetchWithTimeout(
      `${baseUrl}${CLOUD_ENROLL_PATH}`,
      {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "x-mode-enroll-token": enrollmentToken,
        },
        body: JSON.stringify({
          account: {
            provider: "weixin",
            providerUserId,
            ...(displayName ? { displayName } : {}),
          },
          device: {
            id: identity.deviceId,
            name: hostname() || "Mode",
            platform: process.platform,
            version: MODE_VERSION,
            publicKey: identity.publicKeyPem,
            relayTunnelId: identity.relayTunnelId,
          },
          assertion: {
            payload,
            signature: signCloudEnrollment(identity.privateKeyPem, payload),
          },
          nonce,
          issuedAt,
        }),
      },
      ENROLL_TIMEOUT_MS,
    );
    if (!response.ok) {
      let message = `绑定失败（HTTP ${response.status}）`;
      try {
        const body = (await response.json()) as { error?: unknown };
        if (typeof body.error === "string" && body.error) {
          message = `绑定失败：${body.error}`;
        }
      } catch {
        // 保留 HTTP 状态错误。
      }
      throw new CloudApiError(message, response.status);
    }
    const body = (await response.json()) as {
      accountId?: unknown;
      deviceId?: unknown;
      deviceToken?: unknown;
    };
    if (
      typeof body.accountId !== "string" ||
      !body.accountId ||
      typeof body.deviceToken !== "string" ||
      !body.deviceToken
    ) {
      throw new Error("绑定响应缺少账号或设备令牌");
    }
    await credentials.save(CLOUD_ACCOUNT_ID_CREDENTIAL_KEY, body.accountId);
    await credentials.save(CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY, body.deviceToken);
    log(`cloud_account_bound account=${body.accountId.slice(0, 8)}…`);
    const state = await readState();
    onDidChangeEmitter.fire(state);
    return state;
  };

  const autoProvision = startCloudAutoProvision({
    services,
    credentials,
    enrollmentToken,
    readBoundBot,
    bind,
    syncBot: (botId) => cloudBotSync.sync(botId),
    log,
  });

  const listDevices = async (): Promise<CloudDeviceSummary[]> => {
    const token = (await credentials.load(CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY))?.trim();
    if (!token) {
      throw new Error("尚未绑定 Mode Cloud");
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
    bind,
    beginWechatLogin: () => wechatLogin.begin(),
    pollWechatLogin: () => wechatLogin.poll(),
    cancelWechatLogin: () => wechatLogin.cancel(),
    listCloudBotCandidates: () => cloudBotSync.list(),
    syncCloudLoginBot: (params) => cloudBotSync.sync(params.botId),
    async unbind() {
      await credentials.delete(CLOUD_ACCOUNT_ID_CREDENTIAL_KEY);
      await credentials.delete(CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY);
      await credentials.delete(CLOUD_RELAY_TOKEN_CREDENTIAL_KEY);
      const state = await readState();
      onDidChangeEmitter.fire(state);
      return state;
    },
    listDevices,
    pairDevice,
    onDidChange: onDidChangeEmitter.event,
    async dispose() {
      autoProvision.dispose();
      onDidChangeEmitter.dispose();
    },
  };
  return service;
}
