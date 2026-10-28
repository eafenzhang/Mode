import type { SessionDebugSnapshot } from "@mode/shared";
import type { ConversationTurnRenderUnit } from "@/v4/conversationTurnRenderUnits.js";

type DebugRound = SessionDebugSnapshot["rounds"][number];

export interface ConversationTurnWindow {
  turnId: string;
  startedAt: number;
  endedAt?: number;
}

export interface ReplySpeed {
  prefillTokensPerSecond: number | null;
  decodeTokensPerSecond: number | null;
}

const numberFormatterCache = new Map<string, Intl.NumberFormat>();

export function formatReplySpeed(value: number, locale: string): string {
  let formatter = numberFormatterCache.get(locale);
  if (!formatter) {
    formatter = new Intl.NumberFormat(locale, { maximumFractionDigits: 1 });
    numberFormatterCache.set(locale, formatter);
  }
  return formatter.format(value);
}

export function collectConversationTurnWindows(
  units: readonly ConversationTurnRenderUnit[],
): ConversationTurnWindow[] {
  return units.flatMap((unit) => {
    const startedAt = unit.header?.startedAt ?? unit.startedAt;
    if (startedAt === undefined) return [];
    return [
      {
        turnId: unit.turnId,
        startedAt,
        ...(unit.header?.endedAt !== undefined ? { endedAt: unit.header.endedAt } : {}),
      },
    ];
  });
}

/**
 * 一轮可能有多次模型请求（工具调用）；回复速度取该轮时间窗内最后一次带速度的请求。
 * 补充读取/迟到事件落在窗外，不参与本轮展示。
 */
export function selectReplySpeedForTurn(
  rounds: readonly DebugRound[],
  startedAt: number,
  endedAt: number | undefined,
): ReplySpeed | null {
  if (endedAt === undefined || endedAt < startedAt) return null;
  for (let index = rounds.length - 1; index >= 0; index -= 1) {
    const round = rounds[index];
    if (!round || round.recordedAt < startedAt || round.recordedAt > endedAt) continue;
    const decodeTokensPerSecond = round.tokensPerSecond ?? null;
    const prefillTokensPerSecond = round.prefillTokensPerSecond ?? null;
    if (decodeTokensPerSecond === null && prefillTokensPerSecond === null) continue;
    return { prefillTokensPerSecond, decodeTokensPerSecond };
  }
  return null;
}
