import { create } from "zustand";
import { buildTaskWorkspaceKey } from "@/lib/taskQueryCache.js";

/**
 * 远程工作区对话记录镜像的同步状态（per workspaceKey，纯 UI 态）。
 *
 * 镜像产物是**真会话**（导入到目标 scope，见 docs/specs/remote-workspace-conversation-mirror.md），
 * 会话本身由任务列表承载，这里只保留「这一轮同步怎么样」——用来在侧栏给出降级提示与重试入口。
 */
export type ConversationMirrorStatus = "idle" | "syncing" | "ready" | "unsupported" | "error";

export interface WorkspaceConversationMirrorState {
  status: ConversationMirrorStatus;
  /** unsupported / error 时的原始文案（诊断用；UI 展示固定文案）。 */
  error: string;
  lastSyncedAt: number;
  importedCount: number;
}

const EMPTY_STATE: WorkspaceConversationMirrorState = {
  status: "idle",
  error: "",
  lastSyncedAt: 0,
  importedCount: 0,
};

interface ConversationMirrorStoreState {
  byWorkspaceKey: Record<string, WorkspaceConversationMirrorState>;
  setMirrorSyncing: (workspacePath: string, workspaceIdentity?: string) => void;
  setMirrorResult: (
    workspacePath: string,
    workspaceIdentity: string | undefined,
    result: { importedCount: number; syncedAt: number },
  ) => void;
  setMirrorUnsupported: (
    workspacePath: string,
    workspaceIdentity: string | undefined,
    error: unknown,
  ) => void;
  setMirrorError: (
    workspacePath: string,
    workspaceIdentity: string | undefined,
    error: unknown,
  ) => void;
  clearWorkspaceMirror: (workspacePath: string, workspaceIdentity?: string) => void;
}

export function selectWorkspaceConversationMirror(
  state: ConversationMirrorStoreState,
  workspacePath: string,
  workspaceIdentity?: string,
): WorkspaceConversationMirrorState {
  return (
    state.byWorkspaceKey[buildTaskWorkspaceKey(workspacePath, workspaceIdentity)] ?? EMPTY_STATE
  );
}

function resolveErrorMessage(error: unknown): string {
  if (error instanceof Error && error.message.trim()) {
    return error.message;
  }
  return String(error);
}

export const useConversationMirrorStore = create<ConversationMirrorStoreState>()((set) => {
  const patch = (
    workspacePath: string,
    workspaceIdentity: string | undefined,
    next: Partial<WorkspaceConversationMirrorState>,
  ): void => {
    const key = buildTaskWorkspaceKey(workspacePath, workspaceIdentity);
    set((state) => ({
      byWorkspaceKey: {
        ...state.byWorkspaceKey,
        [key]: { ...(state.byWorkspaceKey[key] ?? EMPTY_STATE), ...next },
      },
    }));
  };

  return {
    byWorkspaceKey: {},
    setMirrorSyncing: (workspacePath, workspaceIdentity) => {
      patch(workspacePath, workspaceIdentity, { status: "syncing", error: "" });
    },
    setMirrorResult: (workspacePath, workspaceIdentity, result) => {
      patch(workspacePath, workspaceIdentity, {
        status: "ready",
        error: "",
        importedCount: result.importedCount,
        lastSyncedAt: result.syncedAt,
      });
    },
    setMirrorUnsupported: (workspacePath, workspaceIdentity, error) => {
      patch(workspacePath, workspaceIdentity, {
        status: "unsupported",
        error: resolveErrorMessage(error),
      });
    },
    setMirrorError: (workspacePath, workspaceIdentity, error) => {
      patch(workspacePath, workspaceIdentity, {
        status: "error",
        error: resolveErrorMessage(error),
      });
    },
    clearWorkspaceMirror: (workspacePath, workspaceIdentity) => {
      const key = buildTaskWorkspaceKey(workspacePath, workspaceIdentity);
      set((state) => {
        if (!state.byWorkspaceKey[key]) {
          return state;
        }
        const next = { ...state.byWorkspaceKey };
        delete next[key];
        return { byWorkspaceKey: next };
      });
    },
  };
});
