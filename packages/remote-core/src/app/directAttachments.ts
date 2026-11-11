import type { ControlFrame, DirectAssignment } from "../domain/controlFrames.js";
import { RemoteError } from "../domain/errors.js";
import type { RemoteSocket } from "./controlTypes.js";

/** Client 的直连尝试：失败只回调 onError 并返回 null，由调用方回落 Relay。 */
export async function tryOpenDirectSocket(params: {
  direct?: DirectAssignment;
  connectionId: string;
  open?: (direct: DirectAssignment, connectionId: string) => Promise<RemoteSocket>;
  onError?: (error: RemoteError) => void;
}): Promise<RemoteSocket | null> {
  const { direct, connectionId, open } = params;
  if (!direct || !open) return null;
  try {
    return await open(direct, connectionId);
  } catch (error) {
    params.onError?.(
      new RemoteError("direct_failed", "LAN 直连失败，回落 Relay", { cause: error }),
    );
    return null;
  }
}

export interface PendingDirect {
  ticket: string;
  promise: Promise<RemoteSocket>;
  resolve: (socket: RemoteSocket) => void;
  reject: (error: Error) => void;
  timer: NodeJS.Timeout;
  socket?: RemoteSocket;
}

/**
 * Host 侧 LAN 直连 socket 的待接登记：ticket 由 Control 下发，socket 由
 * `/ws/v1/direct` 首帧带回来。`ControlLink` 只负责选路，登记/超时/清理集中在这里。
 */
export class DirectAttachments {
  private readonly pending = new Map<string, PendingDirect>();
  private readonly timeoutMs: number;
  private readonly send: (
    socket: RemoteSocket,
    frame: { type: ControlFrame["type"]; payload: unknown },
  ) => void;

  constructor(options: {
    timeoutMs: number;
    send: (
      socket: RemoteSocket,
      frame: { type: ControlFrame["type"]; payload: unknown },
    ) => void;
  }) {
    this.timeoutMs = options.timeoutMs;
    this.send = options.send;
  }

  register(connectionId: string, ticket: string): void {
    let resolve: (socket: RemoteSocket) => void = () => undefined;
    let reject: (error: Error) => void = () => undefined;
    const promise = new Promise<RemoteSocket>((res, rej) => {
      resolve = res;
      reject = rej;
    });
    // 超时由选用方处理；这里吞掉 rejection，避免未 await 时变成 unhandled。
    promise.catch(() => undefined);
    const timer = setTimeout(() => {
      this.pending.delete(connectionId);
      reject(new Error("direct socket attach timeout"));
    }, this.timeoutMs);
    timer.unref?.();
    this.pending.set(connectionId, { ticket, promise, resolve, reject, timer });
  }

  /** `/ws/v1/direct` 首帧校验后调用；false 表示没有待接连接或 ticket 不匹配。 */
  accept(connectionId: string, ticket: string, socket: RemoteSocket): boolean {
    const pending = this.pending.get(connectionId);
    if (!pending || pending.ticket !== ticket || pending.socket) {
      return false;
    }
    pending.socket = socket;
    clearTimeout(pending.timer);
    socket.onClose(() => {
      if (this.pending.get(connectionId) !== pending) return;
      this.reject(connectionId, new Error("direct socket closed before selection"));
    });
    this.send(socket, { type: "relay.ready", payload: { connectionId } });
    pending.resolve(socket);
    return true;
  }

  get(connectionId: string): PendingDirect | undefined {
    return this.pending.get(connectionId);
  }

  /** 选中直连后调用；返回前不会关闭 socket。 */
  take(connectionId: string): void {
    const pending = this.pending.get(connectionId);
    if (!pending) return;
    clearTimeout(pending.timer);
    this.pending.delete(connectionId);
  }

  /** 选中 Relay 或连接关闭时丢弃候选并关闭已到达的 socket。 */
  remove(connectionId: string, reason: string): void {
    const pending = this.pending.get(connectionId);
    if (!pending) return;
    clearTimeout(pending.timer);
    this.pending.delete(connectionId);
    pending.socket?.close(1000, reason);
  }

  reject(connectionId: string, error: Error): void {
    const pending = this.pending.get(connectionId);
    if (!pending) return;
    clearTimeout(pending.timer);
    this.pending.delete(connectionId);
    pending.socket?.close(1000, "direct rejected");
    pending.reject(error);
  }

  closeAll(error: Error): void {
    for (const connectionId of this.pending.keys()) {
      this.reject(connectionId, error);
    }
  }
}
