/**
 * Mode Cloud 控制面运行时：SQLite + 控制 API 选项 + 云端微信 Bot。
 *
 * Bot token 的来源优先级：
 *   1. 桌面通过 /v1/admin/wechat-bot 上传（AES-GCM 加密存 SQLite）；
 *   2. 启动时的 MODE_CLOUD_WECHAT_TOKEN（bootstrap/手工配置）。
 * 上传/启动都会重启轮询，保证同一 token 只有一个 poller。
 */
import { createHmac } from "node:crypto";

import { startCloudBot, type CloudBotHandle } from "./cloudBot.js";
import { decryptSecret, encryptSecret } from "./cloudCrypto.js";
import { CloudStore } from "./cloudStore.js";
import type { ControlApiOptions } from "./controlHttp.js";

export interface RelayCloudOptions {
  dbPath?: string;
  enrollToken?: string;
  accountPepper?: string;
  /** 云端微信 Bot：独立 iLink bot token；设置后启用 /v1/auth/*。 */
  wechatToken?: string;
  /** 允许登录的微信用户 ID（原值）；缺省 = TOFU 首个有效登录码。 */
  wechatAllowedUser?: string;
  /** 测试/私有部署的 iLink API 基址。 */
  wechatBaseUrl?: string;
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
  log: (line: string) => void;
}): CloudRuntime | null {
  const enrollToken = params.cloud.enrollToken?.trim();
  if (!enrollToken) {
    return null;
  }
  const store = new CloudStore(params.cloud.dbPath?.trim() || ":memory:");
  const accountPepper = (params.cloud.accountPepper ?? enrollToken).trim();
  let bot: CloudBotHandle | null = null;
  let wechatLoginEnabled = false;

  const hashUser = (userId: string): string =>
    createHmac("sha256", accountPepper).update(`weixin:${userId}`).digest("hex");

  const startBot = (token: string, allowedUserHash?: string): void => {
    bot?.close();
    bot = startCloudBot({
      store,
      token,
      accountPepper,
      ...(allowedUserHash ? { allowedUserHash } : {}),
      ...(params.cloud.wechatBaseUrl ? { baseUrl: params.cloud.wechatBaseUrl } : {}),
      log: params.log,
    });
    wechatLoginEnabled = true;
    params.log("[relay] cloud wechat login enabled");
  };

  const setWechatBot = async (input: {
    token: string;
    allowedUser?: string;
  }): Promise<void> => {
    const encrypted = encryptSecret(accountPepper, input.token);
    const allowedUserHash = input.allowedUser ? hashUser(input.allowedUser) : null;
    store.saveBotConfig({
      tokenCiphertext: encrypted.ciphertext,
      tokenIv: encrypted.iv,
      tokenTag: encrypted.tag,
      allowedUserHash,
      now: Date.now(),
    });
    startBot(input.token, allowedUserHash ?? undefined);
  };

  const control: ControlApiOptions = {
    store,
    enrollToken,
    accountPepper,
    publicBaseUrl: params.publicBaseUrl,
    relayToken: params.relayToken,
    isWechatLoginEnabled: () => wechatLoginEnabled,
    setWechatBot,
    getTunnelIds: params.getTunnelIds,
    log: params.log,
  };
  params.log("[relay] cloud control API enabled");

  const stored = store.loadBotConfig();
  if (stored) {
    try {
      startBot(
        decryptSecret(accountPepper, {
          ciphertext: stored.tokenCiphertext,
          iv: stored.tokenIv,
          tag: stored.tokenTag,
        }),
        stored.allowedUserHash ?? undefined,
      );
    } catch {
      params.log("[relay] cloud bot token decrypt failed; wechat login disabled");
    }
  } else {
    const wechatToken = params.cloud.wechatToken?.trim();
    if (wechatToken) {
      const allowedUser = params.cloud.wechatAllowedUser?.trim();
      startBot(wechatToken, allowedUser ? hashUser(allowedUser) : undefined);
    }
  }

  return {
    control,
    close() {
      bot?.close();
      store.close();
    },
  };
}
