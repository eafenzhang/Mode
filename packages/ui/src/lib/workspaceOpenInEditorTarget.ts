import { createOpenInEditorRemoteTarget, type OpenInEditorRemoteTarget } from "@mode/shared";
import { isWorkspaceTab, type WindowTabState, type WorkspaceTabState } from "@/store/tabStore.js";

export interface WorkspaceOpenInEditorScope {
  workspacePath?: string;
  workspaceIdentity?: string;
  workspaceRemoteSessionId?: string;
}

export interface WorkspaceOpenInEditorTarget {
  isRemoteWorkspace: boolean;
  remoteTarget?: OpenInEditorRemoteTarget;
}

/**
 * 把工作区 tab 的远端目标收敛成「打开方式」可用的本机编辑器目标。
 *
 * 不支持的远端 kind（v3）由 `createOpenInEditorRemoteTarget` 返回 undefined，
 * 这里保持 `isRemoteWorkspace = true`：调用方据此隐藏本机编辑器入口，
 * 而不是在渲染期抛错击穿错误边界。
 */
export function resolveWorkspaceOpenInEditorTarget(
  tabs: readonly WindowTabState[],
  scope: WorkspaceOpenInEditorScope,
): WorkspaceOpenInEditorTarget {
  if (!scope.workspacePath) {
    return { isRemoteWorkspace: false, remoteTarget: undefined };
  }

  const requestedIdentity = scope.workspaceIdentity?.trim() || undefined;
  const requestedRemoteSessionId = scope.workspaceRemoteSessionId?.trim() || undefined;
  const workspaceKey = requestedIdentity || scope.workspacePath;
  const matches = tabs.filter((tab): tab is WorkspaceTabState => {
    if (!isWorkspaceTab(tab) || tab.workspacePath !== scope.workspacePath) {
      return false;
    }

    const tabWorkspaceKey = tab.workspaceIdentity?.trim() || tab.workspacePath;
    const tabRemoteSessionId = tab.remoteSessionId?.trim() || undefined;
    return (
      (!requestedIdentity || tabWorkspaceKey === workspaceKey) &&
      (!requestedRemoteSessionId || tabRemoteSessionId === requestedRemoteSessionId)
    );
  });
  // 远程文件动作以前只携带 Linux path，renderer 无法判断它属于哪个 SSH/WSL 目标；
  // identity/session 已提供时精确匹配，旧调用仅在工作区匹配唯一时提取既有脱敏目标。
  const matchedTab = matches.length === 1 ? matches[0] : undefined;
  const hasRemoteMatch = matches.some((tab) =>
    Boolean(tab.workspaceIdentity || tab.remoteSessionId || tab.remoteTarget),
  );
  const remoteTarget = matchedTab?.remoteTarget;
  return {
    isRemoteWorkspace: hasRemoteMatch,
    remoteTarget: remoteTarget ? createOpenInEditorRemoteTarget(remoteTarget) : undefined,
  };
}
