import { Bot, Webhook } from "lucide-react";
import type { BotConfig, BotServiceStatus } from "@zcode/shared";
import { ALL_BOT_WORKSPACES, BOT_BIND_CODE_TTL_MS } from "@zcode/shared";
import {
  DingDingChannelIcon,
  DiscordChannelIcon,
  FeishuChannelIcon,
  TelegramChannelIcon,
  WeComChannelIcon,
  WeixinChannelIcon,
} from "@/assets/channel-icons/index.js";
import type { BotProviderEntryId } from "@/botsUi.js";
import { cn } from "@/components/lib/utils.js";

export type BindCodeState = {
  botId: string;
  code: string;
  createdAt: number;
  expiresAt: number;
  ttlMs: number;
};

export type FeishuRegistrationState = {
  botId: string;
  deviceCode: string;
  qrUrl: string;
  qrDataUrl: string | null;
  userCode: string;
  interval: number;
  expiresAt: number;
  domain: "feishu" | "lark";
  pollDomain?: "feishu" | "lark";
  status: "pending" | "success" | "access_denied" | "expired" | "error";
  message?: string;
};

export type WeixinRegistrationState = {
  botId: string;
  qrCode: string;
  qrUrl: string;
  qrDataUrl: string | null;
  interval: number;
  expiresAt: number;
  status: "pending" | "scanned" | "success" | "expired" | "error";
  message?: string;
};

export type WecomRegistrationState = {
  botId: string;
  /** 轮询凭据（企业微信 QR 接口的 scode） */
  scode: string;
  authUrl: string;
  qrDataUrl: string | null;
  interval: number;
  expiresAt: number;
  status: "pending" | "scanned" | "expired" | "cancelled" | "denied" | "error";
  message?: string;
};

export const BIND_CODE_TTL_MS = BOT_BIND_CODE_TTL_MS;
export const TELEGRAM_BOTFATHER_URL = "https://t.me/BotFather";

export function isAllWorkspacesAllowed(allowedWorkspaces: readonly string[]): boolean {
  return allowedWorkspaces.length === 0 || allowedWorkspaces.includes(ALL_BOT_WORKSPACES);
}

export function formatBotDisplayName(name: string, fallbackName: string): string {
  return name.trim() || fallbackName;
}

/**
 * 各渠道图标的原始资源自带不同留白（可见内容只占 128 画布的 75%–85%），
 * 同一尺寸并排时视觉大小会不一致。这里按"可见内容统一占 92%"做等比缩放：
 * 系数 = 0.92 × 128 ÷ 资源内可见内容的最大边（实测值，见下方注释）。
 * 只改渲染尺寸，不改动原始图标文件本身。
 */
const CHANNEL_ICON_SCALE: Record<string, string> = {
  // telegram 内容 100/128
  telegram: "scale-[1.18]",
  // feishu / lark 内容 98/128
  feishu: "scale-[1.2]",
  lark: "scale-[1.2]",
  // 钉钉 96/128
  dingding: "scale-[1.23]",
  dingtalk: "scale-[1.23]",
  // 企业微信 102/128
  wecom: "scale-[1.16]",
  // 微信 109/128
  weixin: "scale-[1.08]",
  // Discord 102/128
  discord: "scale-[1.16]",
};

export function ProviderIcon({
  provider,
  className,
}: {
  provider?: BotProviderEntryId | "new";
  className?: string;
}) {
  const iconSrc =
    provider === "telegram"
      ? TelegramChannelIcon
        : provider === "weixin"
          ? WeixinChannelIcon
          : provider === "feishu" || provider === "lark"
            ? FeishuChannelIcon
            : provider === "dingding" || provider === "dingtalk"
              ? DingDingChannelIcon
              : provider === "discord"
                ? DiscordChannelIcon
                : provider === "wecom"
                  ? WeComChannelIcon
                  : null;

  if (iconSrc) {
    return (
      <img
        src={iconSrc}
        alt=""
        aria-hidden="true"
        className={cn(
          "size-4 object-contain",
          provider ? CHANNEL_ICON_SCALE[provider] : undefined,
          className,
        )}
      />
    );
  }

  if (provider === "webhook") {
    return <Webhook className={cn("size-4", className)} />;
  }
  return <Bot className={cn("size-4", className)} />;
}

export function runtimeText(
  runtime: BotServiceStatus["botRuntime"][number] | undefined,
  enabled: boolean,
  formatRuntimeMessage?: (id: string) => string,
): string {
  if (runtime?.messageId && formatRuntimeMessage) {
    return formatRuntimeMessage(runtime.messageId);
  }
  return runtime?.message ?? runtime?.status ?? (enabled ? "enabled" : "disabled");
}

export function runtimeDot(
  runtime: BotServiceStatus["botRuntime"][number] | undefined,
  enabled: boolean,
): string {
  if (runtime?.status === "error") return "bg-destructive";
  if (runtime?.status === "polling" || runtime?.status === "connected") return "bg-success";
  if (enabled) return "bg-foreground-subtle";
  return "bg-border";
}

export function formatBindCountdown(ms: number): string {
  return `${Math.max(0, Math.ceil(ms / 1000))}s`;
}

export function createDefaultCommands(): BotConfig["allowedCommands"] {
  return {
    status: true,
    new: true,
    workspace: true,
    model: true,
    mode: true,
    thoughtLevel: true,
    reply: true,
  };
}
