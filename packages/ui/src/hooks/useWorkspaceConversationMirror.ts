import { useCallback, useEffect, useRef, useState } from "react";
import type { IServiceAccessor } from "@mode/services";
import { runConversationMirrorSync } from "@/lib/conversationMirrorSync.js";
import { buildTaskWorkspaceKey } from "@/lib/taskQueryCache.js";
import { logger } from "@/logger.js";
import {
  selectWorkspaceConversationMirror,
  useConversationMirrorStore,
  type WorkspaceConversationMirrorState,
} from "@/store/conversationMirrorStore.js";

/**
 * 工作区事件触发的重同步防抖窗口：一轮同步会写目标 scope，写本身又发事件，
 * 没有防抖会自激；有防抖后「写完 → 事件 → 一轮空转的同步 → 收敛」。
 */
const MIRROR_RESYNC_DEBOUNCE_MS = 1_200;

/**
 * 相邻两轮的间隔下限。防抖只压瞬时抖动，压不住「水位不前进」的自激：镜像写入的
 * task meta `updatedAt` 依赖对端 CLI 透传 `importedHistory.updatedAt`
 * （`resolveImportedHistoryTimestamps`），对端是旧版本时会退回 `createdAt`，
 * 于是每轮都判定「来源更新」→ 再导 → 再发事件。间隔下限把最坏情况收敛成低频空转，
 * 而不是持续重导。
 */
const MIRROR_RESYNC_MIN_INTERVAL_MS = 20_000;

/**
 * 远程工作区对话记录镜像的同步入口（docs/specs/remote-workspace-conversation-mirror.md）。
 *
 * 触发时机：
 * - `enabled`（远端连接就绪）后跑一轮；同一 workspaceKey + 连接代次 + 窗口只跑一次；
 * - `windowSize` 变大（用户在侧栏「展开更多」）时按新窗口重跑，导入量随可见列表增长；
 * - 两个来源 scope 出现 `workspace_task_list_changed` 时防抖重跑，让两端新发起的对话自动传过去
 *   （只挂载时跑一轮的话，连接之后任一端新开的对话永远到不了另一端）；相邻两轮有间隔下限，
 *   避免镜像自身的写入触发无休止重跑。
 *
 * 同步失败只落到 store 状态（不抛出），镜像不阻断主链路。
 */
export function useWorkspaceConversationMirror(params: {
  workspacePath: string;
  workspaceIdentity?: string;
  /** 当前视图 scope 的服务（远端 tab 为对端代理）。 */
  viewServices: IServiceAccessor;
  /** 本机 host 服务（读本机本地 scope 记录）。 */
  localServices: IServiceAccessor;
  isRemoteTarget: boolean;
  /** 远端连接代次；重连后需要重新同步。 */
  remoteSessionId?: string | null;
  /** 侧栏当前可见的会话条数：镜像同步窗口（默认 5，展开更多后 +5）。 */
  windowSize: number;
  enabled: boolean;
}): { state: WorkspaceConversationMirrorState; refresh: () => void } {
  const {
    workspacePath,
    workspaceIdentity,
    viewServices,
    localServices,
    isRemoteTarget,
    windowSize,
    enabled,
  } = params;
  const remoteSessionId = params.remoteSessionId ?? "";
  const [refreshNonce, setRefreshNonce] = useState(0);
  const lastRunKeyRef = useRef<string | null>(null);
  const runIdRef = useRef(0);
  const resyncTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const lastRunStartedAtRef = useRef(0);

  const state = useConversationMirrorStore((store) =>
    selectWorkspaceConversationMirror(store, workspacePath, workspaceIdentity),
  );

  useEffect(() => {
    if (!enabled) {
      return;
    }
    const runKey = `${buildTaskWorkspaceKey(workspacePath, workspaceIdentity)}::${remoteSessionId}::${windowSize}::${refreshNonce}`;
    if (lastRunKeyRef.current === runKey) {
      return;
    }
    lastRunKeyRef.current = runKey;
    lastRunStartedAtRef.current = Date.now();
    const runId = (runIdRef.current += 1);
    const isCurrentRun = (): boolean => runIdRef.current === runId;

    const store = useConversationMirrorStore.getState();
    store.setMirrorSyncing(workspacePath, workspaceIdentity);

    void runConversationMirrorSync({
      workspacePath,
      viewServices,
      ...(workspaceIdentity ? { viewWorkspaceIdentity: workspaceIdentity } : {}),
      localServices,
      isRemoteTarget,
      windowSize,
      onWarning: (message, error) => {
        // 必须走 lifecycle：logger.warn 在生产构建里被门控成 no-op，镜像的失败诊断会整条丢失，
        // 排查时「日志里没有镜像告警」会被误读成「没跑过」（见 packages/ui/src/logger.ts）。
        logger.lifecycle.warn("[conversation-mirror]", message, error);
      },
    })
      .then((result) => {
        if (!isCurrentRun()) {
          return;
        }
        const current = useConversationMirrorStore.getState();
        if (result.status === "unsupported") {
          current.setMirrorUnsupported(workspacePath, workspaceIdentity, result.error);
          return;
        }
        if (result.status === "degraded") {
          current.setMirrorDegraded(workspacePath, workspaceIdentity, result.error);
          return;
        }
        current.setMirrorResult(workspacePath, workspaceIdentity, {
          importedCount: result.importedIntoView + result.importedIntoPeerLocal,
          syncedAt: Date.now(),
        });
      })
      .catch((error: unknown) => {
        if (!isCurrentRun()) {
          return;
        }
        logger.lifecycle.warn("[conversation-mirror] 同步失败", error);
        useConversationMirrorStore
          .getState()
          .setMirrorError(workspacePath, workspaceIdentity, error);
      });
  }, [
    enabled,
    workspacePath,
    workspaceIdentity,
    viewServices,
    localServices,
    isRemoteTarget,
    remoteSessionId,
    windowSize,
    refreshNonce,
  ]);

  // 两个来源 scope 的变更事件 → 防抖重跑。
  // 只订阅「对端本地 scope」与「本机在该工作区的视图 scope」：前者是①方向的来源，
  // 后者是②方向的来源；任一端新开 / 续聊对话都会在这两个 scope 上产生事件。
  useEffect(() => {
    if (!enabled || !isRemoteTarget) {
      return;
    }
    const modeTaskService = viewServices.modeTaskService;
    if (typeof modeTaskService.onDynamicWorkspaceEvent !== "function") {
      return;
    }
    const scopes = [
      { workspacePath },
      ...(workspaceIdentity ? [{ workspacePath, workspaceIdentity }] : []),
    ];
    const disposables: Array<{ dispose(): void }> = [];
    try {
      for (const scope of scopes) {
        disposables.push(
          modeTaskService.onDynamicWorkspaceEvent(scope)((event) => {
            if (event.type !== "workspace_task_list_changed") {
              return;
            }
            if (resyncTimerRef.current !== null) {
              clearTimeout(resyncTimerRef.current);
            }
            const sinceLastRun = Date.now() - lastRunStartedAtRef.current;
            const delay = Math.max(
              MIRROR_RESYNC_DEBOUNCE_MS,
              MIRROR_RESYNC_MIN_INTERVAL_MS - sinceLastRun,
            );
            resyncTimerRef.current = setTimeout(() => {
              resyncTimerRef.current = null;
              setRefreshNonce((nonce) => nonce + 1);
            }, delay);
          }),
        );
      }
    } catch (error) {
      logger.lifecycle.warn("[conversation-mirror] 订阅工作区事件失败，退化为按需刷新", error);
      for (const disposable of disposables) {
        disposable.dispose();
      }
      return;
    }

    return () => {
      if (resyncTimerRef.current !== null) {
        clearTimeout(resyncTimerRef.current);
        resyncTimerRef.current = null;
      }
      for (const disposable of disposables) {
        disposable.dispose();
      }
    };
  }, [enabled, isRemoteTarget, viewServices, workspacePath, workspaceIdentity]);

  const refresh = useCallback(() => {
    setRefreshNonce((nonce) => nonce + 1);
  }, []);

  return { state, refresh };
}
