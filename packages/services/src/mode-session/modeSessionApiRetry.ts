import {
  normalizeModeApiRetryStatus,
  isModeModelRetryRecoveryProgressPayload,
  resolveWorkspaceKey,
  type ModeSessionApiRetryStatus,
  type ModeSessionStateSnapshot,
  modeApiRetryFromModelNetworkStatusPayload,
  modeApiRetryFromStreamRecoveryPayload,
} from "@mode/shared";
import type { ModeSessionServiceEvent, ModeTaskTarget } from "#src/mode-session/modeSession.js";

export function createModeSessionApiRetryRuntimeTracker(): {
  trackApiRetryFromSessionEvent: (
    params: ModeTaskTarget,
    event: ModeSessionServiceEvent,
  ) => ModeSessionServiceEvent;
  withApiRetryRuntime: (snapshot: ModeSessionStateSnapshot) => ModeSessionStateSnapshot;
} {
  const apiRetryBySessionKey = new Map<string, ModeSessionApiRetryStatus | null>();

  function trackApiRetryFromSessionEvent(
    params: ModeTaskTarget,
    event: ModeSessionServiceEvent,
  ): ModeSessionServiceEvent {
    if (
      event.type === "session.event" &&
      (event.event.type === "session.updated" || event.event.type === "streamRecovery.updated")
    ) {
      // desktop-continuous 的 snapshot runtime 需要跟住 core recovery 进度；
      // 否则切回正在恢复的会话时只能看到空的重试状态。
      const apiRetry = apiRetryFromSessionPayload(asRecord(event.event.payload));
      const key = sessionKey({
        workspacePath: params.workspacePath,
        workspaceIdentity: params.workspaceIdentity,
        sessionId: event.event.sessionId,
      });
      if (apiRetry !== undefined) {
        apiRetryBySessionKey.set(key, apiRetry);
      } else if (
        apiRetryBySessionKey.get(key) != null &&
        isModeModelRetryRecoveryProgressPayload(asRecord(event.event.payload))
      ) {
        // snapshot runtime 和 live UI 要使用同一个恢复成功边界；
        // retry attempt 开始不清，等首个模型进展到达才清，避免重连时状态闪烁或切回 task 后残留。
        apiRetryBySessionKey.set(key, null);
      }
      return event;
    }
    if (event.type === "snapshot") {
      return {
        ...event,
        snapshot: withApiRetryRuntime(event.snapshot),
      };
    }
    return event;
  }

  function withApiRetryRuntime(snapshot: ModeSessionStateSnapshot): ModeSessionStateSnapshot {
    const sessionId = snapshot.session?.sessionId;
    const workspacePath = snapshot.session?.workspace?.workspacePath;
    if (!sessionId || !workspacePath) {
      return snapshot;
    }
    const key = sessionKey({
      workspacePath,
      workspaceIdentity: snapshot.session.workspace.workspaceIdentity,
      sessionId,
    });
    if (snapshot.runtime?.apiRetry !== undefined) {
      apiRetryBySessionKey.set(key, snapshot.runtime.apiRetry);
      return snapshot;
    }
    if (snapshot.session.status === "completed" || snapshot.session.status === "error") {
      apiRetryBySessionKey.set(key, null);
      return {
        ...snapshot,
        runtime: {
          ...snapshot.runtime,
          apiRetry: null,
        },
      };
    }
    if (!apiRetryBySessionKey.has(key)) {
      return snapshot;
    }
    return {
      ...snapshot,
      runtime: {
        ...snapshot.runtime,
        // desktop-continuous 主路径读取 protocol snapshot 时不经过 task adapter。
        // 这里把订阅到的网络重试临时态补回 snapshot，切回运行中 task 时当前 turn 底部才能继续显示重试提示。
        apiRetry: apiRetryBySessionKey.get(key) ?? null,
      },
    };
  }

  function sessionKey(params: ModeTaskTarget): string {
    return `${resolveWorkspaceKey(params)}\u0000${params.sessionId}`;
  }

  return {
    trackApiRetryFromSessionEvent,
    withApiRetryRuntime,
  };
}

function apiRetryFromSessionPayload(
  payload: Record<string, unknown>,
): ModeSessionApiRetryStatus | null | undefined {
  if ("apiRetry" in payload) {
    return normalizeModeApiRetryStatus(payload.apiRetry);
  }
  const runtimeRetry = normalizeModeApiRetryStatus(asRecord(payload.runtime).apiRetry);
  if (runtimeRetry !== undefined) {
    return runtimeRetry;
  }
  const metaRetry = normalizeModeApiRetryStatus(asRecord(asRecord(payload._meta).mode).apiRetry);
  if (metaRetry !== undefined) {
    return metaRetry;
  }
  return (
    modeApiRetryFromStreamRecoveryPayload(payload) ??
    modeApiRetryFromModelNetworkStatusPayload(payload)
  );
}

function asRecord(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}
