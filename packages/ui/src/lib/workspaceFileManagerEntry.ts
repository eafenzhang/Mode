import type { RemoteTarget } from "@mode/shared";

export interface WorkspaceFileManagerEntryInput {
  remoteTarget?: RemoteTarget;
  workspaceIdentity?: string;
  remoteSessionId?: string;
  /** 窄屏远控头部按既有约定省宽度，文件管理器入口一并隐藏。 */
  simplifyForNarrowRemote?: boolean;
}

/**
 * 远端工作区右上角「文件管理器」入口的可见性。
 *
 * 本地工作区不显示：右上角的「打开方式」胶囊已经把 finder/qspace/explorer 排在最前，
 * 主按钮就是系统文件管理器（`sortInstalledEditorsForOpenWith`），再加一个入口是重复。
 * 远端三种标识任一为真即显示，与 SessionPane 既有的 remoteWorkspace 判定同口径。
 *
 * 依据：docs/specs/remote-workspace-file-manager-entry.md「入口可见性」。
 */
export function shouldShowWorkspaceFileManager(input: WorkspaceFileManagerEntryInput): boolean {
  if (input.simplifyForNarrowRemote) return false;
  return Boolean(
    input.remoteTarget || input.workspaceIdentity?.trim() || input.remoteSessionId?.trim(),
  );
}
