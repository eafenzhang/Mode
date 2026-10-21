/**
 * 首次绑定微信 Bot 后自动登录 Mode Cloud 并上传该 Bot。
 *
 * 只在账号尚未绑定（没有 device token）时触发；已绑定后不再自动动其他 Bot，
 * 避免误停用用户的主 Bot。上传后本地 Bot 会被停用（同 token 只能一个 poller）。
 */
import {
  CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY,
  type BotConfig,
  type CloudAccountState,
} from "@mode/shared";
import {
  IBotsService,
  type ICredentialService,
  type ServiceCollection,
} from "@mode/services";

export interface CloudAutoProvisionOptions {
  services: ServiceCollection;
  credentials: ICredentialService;
  enrollmentToken: string;
  readBoundBot: () => Promise<BotConfig | null>;
  bind: () => Promise<CloudAccountState>;
  syncBot: (botId: string) => Promise<CloudAccountState>;
  log: (line: string) => void;
}

export interface CloudAutoProvision {
  dispose(): void;
}

export function startCloudAutoProvision(
  options: CloudAutoProvisionOptions,
): CloudAutoProvision {
  let timer: NodeJS.Timeout | null = null;
  let running = false;

  const run = async (): Promise<void> => {
    if (running) {
      return;
    }
    running = true;
    try {
      if (!options.enrollmentToken) {
        return;
      }
      const deviceToken = (
        await options.credentials.load(CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY)
      )?.trim();
      if (deviceToken) {
        return;
      }
      const bot = await options.readBoundBot();
      if (!bot?.providerUserId?.trim() || !bot.credentialRef?.trim()) {
        return;
      }
      await options.bind();
      await options.syncBot(bot.id);
    } catch (error) {
      options.log(
        `cloud_auto_provision_failed ${error instanceof Error ? error.message : String(error)}`,
      );
    } finally {
      running = false;
    }
  };

  const schedule = (): void => {
    if (timer) {
      clearTimeout(timer);
    }
    timer = setTimeout(() => {
      timer = null;
      void run();
    }, 500);
    timer.unref?.();
  };

  const bots = options.services.getOptional(IBotsService);
  const subscription =
    bots && typeof bots.onDidChange === "function"
      ? bots.onDidChange(() => schedule())
      : null;

  return {
    dispose() {
      subscription?.dispose();
      if (timer) {
        clearTimeout(timer);
        timer = null;
      }
    },
  };
}
