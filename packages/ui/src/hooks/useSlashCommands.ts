/**
 * Mode Agent Slash Commands 便捷 hook
 *
 * 返回当前 workspace 下 Agent 广播的可用 slash commands 列表。
 */
import { useModeSessionStore, selectWorkspaceModeState } from "../store/modeSessionStore.js";

export function useSlashCommands(workspacePath: string, workspaceIdentity?: string) {
  return useModeSessionStore(
    (state) => selectWorkspaceModeState(state, workspacePath, workspaceIdentity).slashCommands,
  );
}
