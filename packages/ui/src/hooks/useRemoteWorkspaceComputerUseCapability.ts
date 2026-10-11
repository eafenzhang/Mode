import { useRemoteWorkspaceSessionStore } from "@/store/remoteWorkspaceSessionStore.js";

/**
 * 当前 workspace 的 LAN 对端电脑控制能力位（attachment 元数据投影）。
 * - undefined：无在册 session 或旧对端未声明（按不支持处理）；
 * - true/false：新对端声明的设备能力（true 才可判定为支持）。
 * ssh/wsl/docker 永不填该位（LAN handle 之外不注入）。
 */
export function useRemoteWorkspaceComputerUseCapability(
  remoteSessionId?: string | null,
  workspaceIdentity?: string | null,
): boolean | undefined {
  return useRemoteWorkspaceSessionStore((state) => {
    const sessionId =
      remoteSessionId?.trim() ||
      (workspaceIdentity?.trim()
        ? state.sessionIdByWorkspaceIdentity[workspaceIdentity.trim()]
        : undefined);
    if (!sessionId) return undefined;
    return state.sessionsById[sessionId]?.capabilities?.computerUse;
  });
}
