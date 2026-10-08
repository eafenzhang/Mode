import type { BotHeartbeatConfig } from "@mode/shared";
import {
  BOT_HEARTBEAT_DEFAULT_INTERVAL_MINUTES,
  BOT_HEARTBEAT_MAX_INTERVAL_MINUTES,
  BOT_HEARTBEAT_MIN_INTERVAL_MINUTES,
} from "@mode/shared";

/**
 * 机器人心跳的纯函数：调度判定与 HEARTBEAT_OK 抑制。
 * 抽成独立模块便于单测——这两条规则决定"什么时候主动打扰用户"和"什么时候保持安静"。
 */

export interface HeartbeatDueState {
  lastHeartbeatAt?: number;
  intervalMinutes: number;
}

/** 归一化心跳配置：缺省值、区间裁剪，保证旧配置也能安全求值。 */
export function normalizeBotHeartbeat(
  config: BotHeartbeatConfig | undefined,
): BotHeartbeatConfig | null {
  if (!config?.enabled) {
    return null;
  }
  const rawInterval = Number.isFinite(config.intervalMinutes)
    ? Math.round(config.intervalMinutes)
    : BOT_HEARTBEAT_DEFAULT_INTERVAL_MINUTES;
  const intervalMinutes = Math.min(
    BOT_HEARTBEAT_MAX_INTERVAL_MINUTES,
    Math.max(BOT_HEARTBEAT_MIN_INTERVAL_MINUTES, rawInterval),
  );
  return { enabled: true, intervalMinutes };
}

/** 距离上次心跳是否已满足间隔（缺失记录视为"应触发"，由调用方决定是否首启立即执行）。 */
export function isHeartbeatDue(now: number, state: HeartbeatDueState): boolean {
  if (!state.lastHeartbeatAt) {
    return true;
  }
  return now - state.lastHeartbeatAt >= state.intervalMinutes * 60 * 1000;
}

/**
 * 心跳回执抑制：整段回复只包含 HEARTBEAT_OK（允许首尾空白与强调符号）时不打扰用户。
 * 只要伴随任何实质内容就照常发送——宁可多发一次，也不吞掉真实汇报。
 */
export function isHeartbeatOkOnly(text: string): boolean {
  const stripped = text
    .replace(/[*_`~\s]+/gu, "")
    .trim()
    .toUpperCase();
  return stripped === "HEARTBEAT_OK" || stripped === "HEARTBEATOK";
}

// 常量迁到 shared（配置语义与 UI 共用），这里转出保持既有引用不变。
export {
  BOT_HEARTBEAT_DEFAULT_INTERVAL_MINUTES,
  BOT_HEARTBEAT_MAX_INTERVAL_MINUTES,
  BOT_HEARTBEAT_MIN_INTERVAL_MINUTES,
};
