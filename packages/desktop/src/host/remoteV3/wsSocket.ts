import { decodeControlFrame, type DirectAssignment, type RemoteSocket } from "@mode/remote-core";
import { WebSocket } from "ws";

const DIRECT_READY_TIMEOUT_MS = 3_000;

/** Node Host 的 ws 适配器；Control/Relay 帧必须保持 text，binary 一律忽略。 */
export function connectWebSocket(url: string): Promise<RemoteSocket> {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url);
    const socket: RemoteSocket = {
      send: (text) => ws.send(text),
      close: (code, reason) => ws.close(code, reason),
      onMessage: (callback) =>
        ws.on("message", (data, isBinary) => {
          if (!isBinary) callback(data.toString());
        }),
      onClose: (callback) =>
        ws.on("close", (code, reason) => callback(code, reason.toString())),
    };
    ws.once("open", () => resolve(socket));
    ws.once("error", reject);
  });
}

/**
 * LAN 直连数据面：连接 Host `/ws/v1/direct`，首发 `direct.attach` 自证 ticket，
 * 等到 `relay.ready` 后才交给 ControlLink。ready 之前的其他消息先缓存，
 * 避免数据帧在 `attachDirect` 挂监听前丢失。
 */
export function connectDirectWebSocket(
  assignment: DirectAssignment,
  connectionId: string,
  address: string,
  timeoutMs = DIRECT_READY_TIMEOUT_MS,
): Promise<RemoteSocket> {
  return new Promise((resolve, reject) => {
    const url = `ws://${address.trim().replace(/\/+$/u, "")}/ws/v1/direct`;
    const ws = new WebSocket(url);
    let settled = false;
    let ready = false;
    const buffered: string[] = [];
    const closeListeners = new Set<(code: number, reason: string) => void>();
    let onMessage: ((text: string) => void) | null = null;
    let closed = false;
    let closeCode = 1006;
    let closeReason = "direct socket closed";

    const fail = (error: Error): void => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      try {
        ws.terminate();
      } catch {
        // 已关闭时忽略。
      }
      reject(error);
    };
    const timer = setTimeout(
      () => fail(new Error(`LAN direct ready timeout (${address})`)),
      timeoutMs,
    );
    timer.unref?.();

    const socket: RemoteSocket = {
      send(text) {
        if (ws.readyState === WebSocket.OPEN) ws.send(text);
      },
      close(code = 1000, reason = "") {
        ws.close(code, reason);
      },
      onMessage(callback) {
        onMessage = callback;
        for (const text of buffered.splice(0, buffered.length)) callback(text);
      },
      onClose(callback) {
        closeListeners.add(callback);
        if (closed) queueMicrotask(() => callback(closeCode, closeReason));
      },
    };

    ws.on("message", (data, isBinary) => {
      if (isBinary) return;
      const text = data.toString();
      if (ready) {
        if (onMessage) onMessage(text);
        else buffered.push(text);
        return;
      }
      try {
        const frame = decodeControlFrame(Buffer.from(text, "utf8"));
        if (frame.type === "relay.ready" && frame.payload.connectionId === connectionId) {
          ready = true;
          settled = true;
          clearTimeout(timer);
          resolve(socket);
          return;
        }
      } catch {
        // 非协议帧：留给后续消费者，由协议层决定是否报错。
      }
      buffered.push(text);
    });
    ws.once("open", () => {
      ws.send(JSON.stringify({ type: "direct.attach", connectionId, ticket: assignment.ticket }));
    });
    ws.once("error", (error) => fail(error));
    ws.once("close", (code, reason) => {
      closed = true;
      closeCode = code;
      closeReason = reason.toString() || `direct ws closed ${code}`;
      fail(new Error(closeReason));
      for (const listener of closeListeners) listener(closeCode, closeReason);
      closeListeners.clear();
    });
  });
}
