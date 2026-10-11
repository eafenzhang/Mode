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

/**
 * 观察的投递来源：`"base"` = 本机既有订阅；`{ sessionId }` = 某个在册 remote session 的订阅。
 *
 * `onDynamicCuaPermissionObservation` 是 host-wide 事件（workspace 字段在负载里、订阅参数
 * 不带 workspace 过滤），对端 session 订阅收到的是**对端 host 的全部观察**，其中包括对端用户
 * 在对端本机 workspace 上跑的 turn（无 identity 或非本会话 identity）。这类观察的动作（状态
 * 查询 / 授权确认 / 打开引导）必须落在拥有它的机器上，绝不能由本地 base 执行——否则本机缺
 * 权限时会因对端的 turn 在本机弹卡、查状态甚至开本机 OS 引导。
 *
 * 判定规则（fail closed）：
 * - base 投递：一律放行，维持既有逐字行为（本机观察 → base + 平台路径；identity 解析到在册
 *   session 的仍按 `resolveCuaPermissionSource` 归属对端服务）。
 * - 对端 session 投递：观察的 `workspaceIdentity` 必须经 `sessionIdByWorkspaceIdentity`
 *   解析回**投递它的那个** session，且该 session 仍在册；identity 缺失（对端本机 workspace
 *   的观察）、归属其他 session、或未绑定的一律丢弃，不弹卡、不查状态、不开引导。
 *   依据：LAN attachment 的 descriptor 必带 workspaceIdentity（desktopRemoteSessions
 *   `attachRendererPort` 缺 identity 直接抛错）且恒为 `remote:lan:` 形态（host
 *   `buildRemoteWorkspaceIdentity` 兜底构造），所以本会话自己的观察不可能是无 identity 形态。
 *
 * docs/specs/lan-remote-computer-use.md 事件顺序 ④。
 */
export function shouldForwardCuaPermissionObservationToSource(input: {
  deliveredBy: "base" | { sessionId: string };
  observation: { workspaceIdentity?: string };
  sessionsById: Record<string, RemoteWorkspaceSession>;
  sessionIdByWorkspaceIdentity: Record<string, string>;
}): boolean {
  if (input.deliveredBy === "base") return true;
  const identity = input.observation.workspaceIdentity?.trim();
  if (!identity) return false;
  if (input.sessionIdByWorkspaceIdentity[identity] !== input.deliveredBy.sessionId) return false;
  return Boolean(input.sessionsById[input.deliveredBy.sessionId]);
}
