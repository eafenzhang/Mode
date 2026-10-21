/**
 * 把本机微信 Bot 同步为“云端登录 Bot”（docs/specs/mode-cloud-account-directory.md）。
 *
 * 上传 token + 已绑定的微信用户到 /v1/admin/wechat-bot；云端加密保存并启动 Bot。
 * 成功后停用本地 Bot：同一条 iLink token 不能有两个 getupdates poller。
 */
import type { BotConfig, CloudAccountState, CloudBotCandidate } from "@mode/shared";
import {
  IBotsService,
  type ICredentialService,
  type ServiceCollection,
} from "@mode/services";

import { fetchWithTimeout } from "./lanPeerTransport.js";

const SYNC_TIMEOUT_MS = 15_000;

export interface CloudBotSyncOptions {
  services: ServiceCollection;
  credentials: ICredentialService;
  baseUrl: string;
  enrollmentToken: string;
  readState: () => Promise<CloudAccountState>;
  onState: (state: CloudAccountState) => void;
  log: (line: string) => void;
}

export interface CloudBotSync {
  list(): Promise<CloudBotCandidate[]>;
  sync(botId: string): Promise<CloudAccountState>;
}

export function createCloudBotSync(options: CloudBotSyncOptions): CloudBotSync {
  const readBots = async (): Promise<BotConfig[]> => {
    const bots = options.services.getOptional(IBotsService);
    return bots ? await bots.listBots() : [];
  };

  return {
    async list() {
      const list = await readBots();
      return list
        .filter((bot) => bot.provider === "weixin")
        .map((bot) => ({
          id: bot.id,
          name: bot.displayName?.trim() || bot.providerUserId?.trim() || bot.id,
          ...(bot.providerUserId?.trim() ? { boundUser: bot.providerUserId.trim() } : {}),
          enabled: bot.enabled,
          hasToken: Boolean(bot.credentialRef?.trim()),
        }));
    },

    async sync(botId) {
      if (!options.enrollmentToken) {
        throw new Error("需要部署 enrollment secret 才能同步云端登录 Bot");
      }
      const bots = options.services.getOptional(IBotsService);
      if (!bots) {
        throw new Error("Bot 服务不可用");
      }
      const list = await bots.listBots();
      const bot = list.find((item) => item.id === botId && item.provider === "weixin");
      if (!bot) {
        throw new Error("找不到该微信 Bot");
      }
      const providerUserId = bot.providerUserId?.trim();
      if (!providerUserId) {
        throw new Error("请先在微信里 /bind 这个 Bot");
      }
      const credentialRef = bot.credentialRef?.trim();
      if (!credentialRef) {
        throw new Error("这个 Bot 缺少 token");
      }
      const token = (await options.credentials.load(credentialRef))?.trim();
      if (!token) {
        throw new Error("这个 Bot 缺少 token");
      }
      const response = await fetchWithTimeout(
        `${options.baseUrl}/v1/admin/wechat-bot`,
        {
          method: "POST",
          headers: {
            "content-type": "application/json",
            "x-mode-enroll-token": options.enrollmentToken,
          },
          body: JSON.stringify({ token, allowedUser: providerUserId }),
        },
        SYNC_TIMEOUT_MS,
      );
      if (!response.ok) {
        throw new Error(`同步云端登录 Bot 失败（HTTP ${response.status}）`);
      }
      // 云端接管后停用本地 Bot，避免两边同时 getupdates。
      await bots.saveBot({ bot: { ...bot, enabled: false } });
      options.log(`cloud_bot_synced bot=${bot.id}`);
      const state = await options.readState();
      options.onState(state);
      return state;
    },
  };
}
