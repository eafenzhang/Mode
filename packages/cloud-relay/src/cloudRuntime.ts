/**
 * Mode Cloud 控制面运行时：SQLite + 控制 API 选项（docs/specs/cloud-personal-accounts.md，v2）。
 *
 * 账号身份：仅 GitHub OAuth（Device Flow，云端验证 access token，JIT 建号）。
 * 控制面在提供 dbPath 时启用；纯中继部署不传 dbPath 即无控制面。
 */
import { CloudStore } from "./cloudStore.js";
import { hashDeviceToken } from "./controlHttp.js";
import { createGithubDeviceFlow } from "./githubDeviceFlow.js";
import { safeEqual } from "./internal.js";
import { loadDbKey } from "./secretBox.js";
import type { ControlApiOptions } from "./controlHttp.js";

export interface RelayCloudOptions {
  dbPath?: string;
  accountPepper?: string;
  /** 设备访问令牌静态加密密钥；未配置时在数据目录自动生成 cloud-db.key。 */
  dbKey?: string;
  /** 人类会话时长（毫秒）；缺省 30 天。 */
  sessionTtlMs?: number;
  /** GitHub API 基址（验证 access token 用）；缺省 https://api.github.com。 */
  githubApiBaseUrl?: string;
  /** 门户 GitHub Device Flow 的 public client_id；缺省则门户 GitHub 登录关闭。 */
  githubClientId?: string;
  /** GitHub device/token 端点基址；缺省 https://github.com（测试/代理用）。 */
  githubBaseUrl?: string;
}

export interface CloudRuntime {
  control: ControlApiOptions;
  close(): void;
}

export function startCloudRuntime(params: {
  cloud: RelayCloudOptions;
  relayToken: string;
  publicBaseUrl: string;
  getTunnelIds: () => string[];
  /** revoke 设备后主动断开该隧道（docs/specs/cloud-personal-accounts.md）。 */
  closeTunnel: (tunnelId: string) => void;
  log: (line: string) => void;
}): CloudRuntime | null {
  const dbPath = params.cloud.dbPath?.trim();
  if (!dbPath) {
    return null;
  }
  const store = new CloudStore(dbPath);
  const dbKey = loadDbKey({ envKey: params.cloud.dbKey, dbPath });

  /**
   * 隧道按设备鉴权：tunnelId 命中目录时校验该设备自己的 relayToken；
   * 未命中/未启用控制面时由 server 回退共享 secret（向后兼容纯中继部署）。
   */
  const verifyTunnelToken = (tunnelId: string, presented: string): boolean => {
    const device = store.findDeviceByTunnelId(tunnelId);
    if (!device || device.revoked || !device.relayTokenHash) {
      return false;
    }
    return safeEqual(device.relayTokenHash, hashDeviceToken(presented));
  };

  const githubClientId = params.cloud.githubClientId?.trim() ?? "";
  const githubDeviceFlow = githubClientId
    ? createGithubDeviceFlow({
        clientId: githubClientId,
        ...(params.cloud.githubBaseUrl?.trim()
          ? { githubBaseUrl: params.cloud.githubBaseUrl.trim() }
          : {}),
      })
    : null;

  const control: ControlApiOptions = {
    store,
    // 锚点 HMAC 的 pepper：优先部署配置，缺省退回共享中继 secret（部署内稳定即可）。
    accountPepper: (params.cloud.accountPepper ?? params.relayToken).trim(),
    publicBaseUrl: params.publicBaseUrl,
    relayToken: params.relayToken,
    getTunnelIds: params.getTunnelIds,
    closeTunnel: params.closeTunnel,
    verifyTunnelToken,
    dbKey,
    githubDeviceFlow,
    ...(params.cloud.sessionTtlMs && params.cloud.sessionTtlMs > 0
      ? { sessionTtlMs: params.cloud.sessionTtlMs }
      : {}),
    ...(params.cloud.githubApiBaseUrl?.trim()
      ? { githubApiBaseUrl: params.cloud.githubApiBaseUrl.trim() }
      : {}),
    log: params.log,
  };
  params.log("[relay] cloud control API enabled");

  return {
    control,
    close() {
      store.close();
    },
  };
}
