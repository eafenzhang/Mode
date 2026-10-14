/**
 * IM 机器人接入三段状态（docs/specs/im-bot-onboarding.md 的 R1 口径）。
 *
 * 修复原因：卡片把「凭据就绪 / 身份绑定 / 运行启动」三段状态散落在圆点、按钮文案和错误面板里，
 * 同一件事有多种说法；微信更明显——`isConnected` 只看凭据，扫码成功即显示绿色的「已连通」，
 * 但此时机器人还收不到消息（要等首条入站消息写入 `weixinActivatedAt` 才算激活）。
 *
 * 这里把三段判定收成纯函数，卡片只负责渲染：判定口径只有一处，且可单测。
 *
 * 微信不设「绑定」段：扫码时平台一并完成身份绑定，`/bind` 对它不适用（spec R2 的不对称是刻意的），
 * 第三段因此换成「激活」。
 */
import type { BotConfig, BotServiceStatus } from "@mode/shared";

export type BotOnboardingStageId = "credentials" | "binding" | "activation" | "runtime";
export type BotOnboardingStageState = "done" | "pending" | "error";

export interface BotOnboardingStage {
  id: BotOnboardingStageId;
  state: BotOnboardingStageState;
}

type BotOnboardingInput = Pick<
  BotConfig,
  | "provider"
  | "credentialRef"
  | "wecomBotId"
  | "dingtalkClientId"
  | "feishuAppId"
  | "providerUserId"
  | "enabled"
>;

/** 平台侧凭据字段：只有它和 credentialRef 同时就位，凭据段才算完成。 */
function hasProviderCredentialField(bot: BotOnboardingInput): boolean {
  switch (bot.provider) {
    case "wecom":
      return Boolean(bot.wecomBotId);
    case "dingtalk":
      return Boolean(bot.dingtalkClientId);
    case "feishu":
    case "lark":
      return Boolean(bot.feishuAppId);
    default:
      return true;
  }
}

function deriveRuntimeStageState(
  enabled: boolean,
  status: BotServiceStatus["botRuntime"][number]["status"] | undefined,
): BotOnboardingStageState {
  if (!enabled) return "pending";
  if (status === "error") return "error";
  if (status === "polling" || status === "connected") return "done";
  return "pending";
}

export function deriveBotOnboardingStages(input: {
  bot: BotOnboardingInput;
  runtimeStatus: BotServiceStatus["botRuntime"][number]["status"] | undefined;
  weixinActivated: boolean;
}): BotOnboardingStage[] {
  const { bot, runtimeStatus, weixinActivated } = input;
  const credentialsReady = Boolean(bot.credentialRef) && hasProviderCredentialField(bot);
  const credentials: BotOnboardingStage = {
    id: "credentials",
    state: credentialsReady ? "done" : "pending",
  };

  if (bot.provider === "weixin") {
    return [credentials, { id: "activation", state: weixinActivated ? "done" : "pending" }];
  }

  return [
    credentials,
    { id: "binding", state: bot.providerUserId ? "done" : "pending" },
    { id: "runtime", state: deriveRuntimeStageState(bot.enabled, runtimeStatus) },
  ];
}

/** 圆点配色沿用 shared.tsx `runtimeDot` 的词汇：就绪绿、待办灰、异常红。 */
export function botOnboardingStageDotClass(state: BotOnboardingStageState): string {
  switch (state) {
    case "done":
      return "bg-success";
    case "error":
      return "bg-destructive";
    default:
      return "bg-foreground-subtle";
  }
}
