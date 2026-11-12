import { RemoteError } from "../domain/errors.js";
import type { ActiveSession } from "./connectionRuntime.js";
import type { DirectAttachments } from "./directAttachments.js";
import type { RemoteSocket } from "./controlTypes.js";

/** Host 收到带直连候选的连接后，等待 Client 选路；超时关闭会话，避免一直挂起。 */
export function markDirectPending(active: ActiveSession, timeoutMs: number): void {
  active.transportSelected = false;
  const timer = setTimeout(() => {
    if (!active.transportSelected) {
      active.session.fail(new RemoteError("transport_timeout", "数据面选路超时"));
    }
  }, timeoutMs + 500);
  timer.unref?.();
}

/** 应用 Client 的选路结果：LAN 直连或 Relay；调用方负责把结果写回 ActiveSession。 */
export async function selectTransport(params: {
  active: ActiveSession;
  transport: "lan" | "p2p" | "turn" | "relay";
  direct: DirectAttachments;
  attachDirect: (active: ActiveSession, socket: RemoteSocket) => void;
  attachRelay: (active: ActiveSession, timeoutMs: number) => Promise<void>;
  attachP2P?: () => Promise<void>;
  timeoutMs: number;
}): Promise<void> {
  const { active, transport, direct } = params;
  if (transport === "lan") {
    const pending = direct.get(active.connectionId);
    if (!pending) throw new RemoteError("direct_required", "连接没有直连候选");
    const socket = await pending.promise;
    direct.take(active.connectionId);
    params.attachDirect(active, socket);
    return;
  }
  if (transport === "p2p") {
    if (!params.attachP2P) throw new RemoteError("p2p_required", "本端不支持 P2P");
    await params.attachP2P();
    return;
  }
  direct.remove(active.connectionId, "relay selected");
  await params.attachRelay(active, params.timeoutMs);
}

/** Host 侧应用选路结果：建立数据面并标记；失败时关闭会话，避免半开连接。 */
export function applyTransportSelection(params: {
  active: ActiveSession;
  transport: "lan" | "p2p" | "turn" | "relay";
  direct: DirectAttachments;
  attachDirect: (active: ActiveSession, socket: RemoteSocket) => void;
  attachRelay: (active: ActiveSession, timeoutMs: number) => Promise<void>;
  attachP2P?: () => Promise<void>;
  timeoutMs: number;
}): void {
  const { active } = params;
  void selectTransport(params)
    .then(() => {
      active.transportSelected = true;
    })
    .catch((error: unknown) => {
      const normalized =
        error instanceof RemoteError
          ? error
          : new RemoteError("transport_failed", "数据面建立失败", { cause: error });
      active.session.fail(normalized);
      params.direct.reject(active.connectionId, normalized);
    });
}
