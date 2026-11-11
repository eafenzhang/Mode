import type { WebControlLink } from "@mode/remote-core/web";
import type { IServiceAccessor } from "@mode/services";

import { openWebWorkspace } from "./webWorkspace.js";

/** 浏览器侧的 v3 远程工作区 attachment（注册进 renderer 的 session store 前的最小形态）。 */
export interface V3WorkspaceSession {
  sessionId: string;
  services: IServiceAccessor;
  dispose(): void;
}

/**
 * 把 v3 Host 的 `/ws/host` 隧道包装成远程工作区 session。
 *
 * 与 `openWebWorkspace` 的分工：本函数只负责 sessionId 与注册顺序；隧道/RPC 生命周期仍在
 * `openWebWorkspace`。注册失败时回收隧道，避免留下未登记的在途连接。
 */
export async function attachV3WorkspaceSession(params: {
  link: Pick<WebControlLink, "request" | "openStream">;
  hostDeviceId: string;
  /** 缺省随机 UUID；跨重连复用同一 id 时由调用方传入。 */
  sessionId?: string;
  register: (session: V3WorkspaceSession) => void;
}): Promise<V3WorkspaceSession> {
  const workspace = await openWebWorkspace({
    link: params.link,
    hostDeviceId: params.hostDeviceId,
  });
  const session: V3WorkspaceSession = {
    sessionId: params.sessionId ?? crypto.randomUUID(),
    services: workspace.services,
    dispose: () => workspace.dispose(),
  };
  try {
    params.register(session);
  } catch (error) {
    session.dispose();
    throw error;
  }
  return session;
}
