import { isInternalConversationWorkspacePath, type ServerRemoteWorkspaceInfo } from "@mode/shared";
import { ISettingService, type ServiceCollection } from "@mode/services";
import { getConversationWorkspaceDir } from "@mode/services/node";

/**
 * 对端在「选择工作区」里可挑选的本机目录：最近打开的本机工作区 + 最近项目，去重后截断（最多 20）。
 *
 * 只暴露用户选择的路径：本机默认对话工作区（数据根内）是内部目录，不外发给局域网对端
 * （spec: docs/specs/lan-paired-devices.md）。Electron Host 与 Tauri Host 共用这一份实现，
 * 避免「两边暴露的目录集合」漂移。
 */
export async function collectLanAccessWorkspaces(
  services: ServiceCollection,
): Promise<ServerRemoteWorkspaceInfo[]> {
  const settingService = services.getOptional(ISettingService);
  const settings = await settingService?.get().catch(() => null);
  const byPath = new Map<string, ServerRemoteWorkspaceInfo>();
  const conversationWorkspaceDir = getConversationWorkspaceDir();
  const caseInsensitive = process.platform === "win32";
  const add = (workspacePath: string, workspaceIdentity?: string) => {
    const trimmed = workspacePath.trim();
    if (!trimmed || byPath.has(trimmed)) {
      return;
    }
    if (
      isInternalConversationWorkspacePath(trimmed, conversationWorkspaceDir, { caseInsensitive })
    ) {
      return;
    }
    const label =
      trimmed
        .replace(/[\\/]+$/u, "")
        .split(/[\\/]/u)
        .pop() || trimmed;
    byPath.set(trimmed, {
      path: trimmed,
      label,
      ...(workspaceIdentity?.trim() ? { workspaceIdentity: workspaceIdentity.trim() } : {}),
    });
  };
  for (const entry of settings?.lastWorkspaceSession ?? []) {
    if (entry.kind !== "local") {
      continue;
    }
    add(entry.workspacePath);
  }
  for (const recent of settings?.recentProjects ?? []) {
    add(recent);
  }
  return [...byPath.values()].slice(0, 20);
}
