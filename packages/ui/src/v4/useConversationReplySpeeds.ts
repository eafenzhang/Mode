import { useEffect, useMemo, useRef, useState } from "react";
import type { SessionDebugSnapshot } from "@mode/shared";
import { useServices } from "@/hooks/useServices.js";
import {
  selectReplySpeedForTurn,
  type ConversationTurnWindow,
  type ReplySpeed,
} from "@/v4/conversationReplySpeedModel.js";

const REPLY_SPEED_INITIAL_DELAY_MS = 300;
const REPLY_SPEED_POLL_MS = 1000;
const REPLY_SPEED_IDLE_RETRY_MS = 800;
const REPLY_SPEED_IDLE_RETRIES = 2;

/**
 * 轮尾速度是按轮只读派生：历史打开时补一次 debug 快照，运行中的轮按秒跟进，
 * 轮完成后停止。旧 CLI / 只读会话没有 session/debug 能力时保持空，不影响消息渲染。
 */
export function useConversationReplySpeeds(input: {
  workspacePath: string;
  workspaceIdentity?: string;
  sessionId?: string | null;
  windows: readonly ConversationTurnWindow[];
}): ReadonlyMap<string, ReplySpeed> {
  const { modeAgentService } = useServices();
  const [rounds, setRounds] = useState<SessionDebugSnapshot["rounds"]>([]);
  const sessionId = input.sessionId;
  const completedCount = input.windows.reduce(
    (count, window) => count + (window.endedAt === undefined ? 0 : 1),
    0,
  );
  const hasRunningTurn = input.windows.some((window) => window.endedAt === undefined);
  const hasTurns = input.windows.length > 0;
  const scopeKey = `${input.workspaceIdentity?.trim() || input.workspacePath}\0${input.sessionId ?? ""}`;
  const windowsRef = useRef(input.windows);
  windowsRef.current = input.windows;
  // 运行中的轮每秒会让 renderUnits 换引用；窗口值不变时不能重建 speed 对象，
  // 否则每个可见历史 turn 都会因新 props 每秒重渲染。
  const windowsKey = input.windows
    .map((window) => `${window.turnId}\0${window.startedAt}\0${window.endedAt ?? ""}`)
    .join("\n");

  useEffect(() => {
    setRounds([]);
  }, [scopeKey]);

  useEffect(() => {
    if (!sessionId || !hasTurns) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let attempts = 0;
    const refresh = async () => {
      try {
        const snapshot = await modeAgentService.readSessionDebug({
          workspacePath: input.workspacePath,
          workspaceIdentity: input.workspaceIdentity,
          sessionId,
        });
        if (!cancelled) setRounds(snapshot.rounds);
      } catch {
        // session/debug 是可选诊断能力；旧 CLI 或只读会话失败时静默保持空。
      }
      if (cancelled) return;
      attempts += 1;
      if (hasRunningTurn || attempts < REPLY_SPEED_IDLE_RETRIES) {
        timer = setTimeout(
          () => void refresh(),
          hasRunningTurn ? REPLY_SPEED_POLL_MS : REPLY_SPEED_IDLE_RETRY_MS,
        );
      }
    };
    timer = setTimeout(() => void refresh(), REPLY_SPEED_INITIAL_DELAY_MS);
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [
    completedCount,
    hasRunningTurn,
    hasTurns,
    sessionId,
    input.workspaceIdentity,
    input.workspacePath,
    modeAgentService,
    scopeKey,
  ]);

  return useMemo(() => {
    const speeds = new Map<string, ReplySpeed>();
    for (const window of windowsRef.current) {
      const speed = selectReplySpeedForTurn(rounds, window.startedAt, window.endedAt);
      if (speed) speeds.set(window.turnId, speed);
    }
    return speeds;
  }, [rounds, windowsKey]);
}
