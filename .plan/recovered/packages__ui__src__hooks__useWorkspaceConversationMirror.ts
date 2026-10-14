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
 * 远程工作区对话记录镜像的同步入口（docs/specs/remote-workspace-conversation-mirror.md）。
 *
 * 触发时机由调用方的 `enabled` 决定：远端连接就绪（rpcReady）时自动跑一轮；同一 workspaceKey +
 * 连接代次只跑一次，避免每次渲染重复导入。同步失败只落到 store 状态（不抛出），镜像不阻断主链路。
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
  enabled: boolean;
}): { state: WorkspaceConversationMirrorState; refresh: () => void } {
  const {
    workspacePath,
    workspaceIdentity,
    viewServices,
    localServices,
    isRemoteTarget,
    enabled,
  } = params;
  const remoteSessionId = params.remoteSessionId ?? "";
  const [refreshNonce, setRefreshNonce] = useState(0);
  const lastRunKeyRef = useRef<string | null>(null);
  const runIdRef = useRef(0);

  const state = useConversationMirrorStore((store) =>
    selectWorkspaceConversationMirror(store, workspacePath, workspaceIdentity),
  );

  useEffect(() => {
    if (!enabled) {
      return;
    }
    const runKey = `${buildTaskWorkspaceKey(workspacePath, workspaceIdentity)}::${remoteSessionId}::${refreshNonce}`;
    if (lastRunKeyRef.current === runKey) {
      return;
    }
    lastRunKeyRef.current = runKey;
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
      onWarning: (message, error) => {
        logger.warn("[conversation-mirror]", message, error);
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
        current.setMirrorResult(workspacePath, workspaceIdentity, {
          importedCount: result.importedIntoView + result.importedIntoPeerLocal,
          syncedAt: Date.now(),
        });
      })
      .catch((error: unknown) => {
        if (!isCurrentRun()) {
          return;
        }
        logger.warn("[conversation-mirror] 同步失败", error);
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
    refreshNonce,
  ]);

  const refresh = useCallback(() => {
    setRefreshNonce((nonce) => nonce + 1);
  }, []);

  return { state, refresh };
}
