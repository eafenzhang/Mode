import type { IServiceAccessor } from "@mode/services";
import type { RemoteWorkspaceSession } from "@/store/remoteWorkspaceSessionStore.js";

/**
 * 权限观察的归属 accessor：identity 解析到在册 remote session 时用对端服务
 * （LAN 新对端才会有该 session 的 capabilities——T5 已保证归属正确），否则本机 base。
 *
 * 判据与 T5 的 merge 归属同键（`typeof capabilities?.computerUse === "boolean"`，
 * false 也归对端，因为状态查询与引导动作都是对端事实）。
 * docs/specs/lan-remote-computer-use.md 接口变更 5、事件顺序 ④。
 */
export function resolveCuaPermissionSource(input: {
  baseServices: IServiceAccessor;
  observation: { workspaceIdentity?: string };
  sessionsById: Record<string, RemoteWorkspaceSession>;
  sessionIdByWorkspaceIdentity: Record<string, string>;
}): { services: IServiceAccessor; remote: boolean } {
  const identity = input.observation.workspaceIdentity?.trim();
  const sessionId = identity ? input.sessionIdByWorkspaceIdentity[identity] : undefined;
  const session = sessionId ? input.sessionsById[sessionId] : undefined;
  if (session && typeof session.capabilities?.computerUse === "boolean") {
    return { services: session.services, remote: true };
  }
  return { services: input.baseServices, remote: false };
}
