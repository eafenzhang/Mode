import type { RemoteStream } from "@mode/remote-core";
import { WebSocket } from "ws";

export interface LocalApiTarget {
  baseUrl: string;
  token: string;
}

const MAX_FRAME_BYTES = 2 * 1024 * 1024;
/**
 * 媒体面背压线（spec: remote-desktop-latency §2.3）：数据面发送队列超过 128KB
 * （≈8Mbps 下 130ms）时丢**二进制**视频帧，文本控制帧照常发送。
 * 这里读的是 v3 数据面的真实水位；被控媒体中继的本地 WS 水位在 v3 下会被
 * wsTunnel 立即吸干，挡不住真正的排队。
 */
const MEDIA_BACKPRESSURE_BYTES = 128 * 1024;
/** WS 未 open 时的待发上限：超出后丢最旧的（媒体优先，控制帧不丢）。 */
const MAX_PENDING_FRAMES = 64;

/**
 * 把 v3 stream 桥接到 Host 本机 LAN 服务的 `/ws*`。
 * 帧格式：首字节 0=text、1=binary，其余为原始 payload；token 只进本机 Cookie。
 */
export function attachLocalWebSocketTunnel(stream: RemoteStream, target: LocalApiTarget): void {
  const path = typeof stream.metadata.path === "string" ? stream.metadata.path : "";
  if (!path.startsWith("/ws")) {
    stream.close("unsupported tunnel path");
    return;
  }
  const url = new URL(path, target.baseUrl);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  const headers: Record<string, string> = {
    cookie: `mode_lite_token=${encodeURIComponent(target.token)}`,
  };
  const extra = stream.metadata.headers;
  if (extra && typeof extra === "object" && !Array.isArray(extra)) {
    for (const [rawName, value] of Object.entries(extra)) {
      const name = rawName.toLowerCase();
      if (name === "cookie" || name === "authorization") continue;
      if (!/^[a-z0-9-]+$/.test(name) || typeof value !== "string") continue;
      if (value.length > 1024 || /[\r\n]/.test(value)) continue;
      headers[name] = value;
    }
  }
  const socket = new WebSocket(url, { headers });
  const pending: Uint8Array[] = [];
  let ready = false;

  const sendFrame = (frame: Uint8Array): void => {
    if (socket.readyState !== WebSocket.OPEN) return;
    const kind = frame[0] ?? 1;
    const payload = Buffer.from(frame.subarray(1));
    socket.send(payload, { binary: kind === 1 });
  };

  stream.onData((frame) => {
    if (frame.length === 0 || frame.length > MAX_FRAME_BYTES) {
      stream.close("invalid tunnel frame");
      socket.close();
      return;
    }
    const binary = (frame[0] ?? 1) === 1;
    if (binary && (stream.bufferedAmount ?? 0) > MEDIA_BACKPRESSURE_BYTES) {
      // 丢最旧视频帧：丢帧后 renderer 检测 seq 缺口并重同步（spec §2.4）。
      return;
    }
    if (!ready) {
      if (pending.length >= MAX_PENDING_FRAMES) {
        // 上限：优先丢二进制（视频），避免开流窗口里无限堆积。
        const dropAt = pending.findIndex((item) => (item[0] ?? 1) === 1);
        if (dropAt >= 0) pending.splice(dropAt, 1);
        else return;
      }
      pending.push(frame);
      return;
    }
    sendFrame(frame);
  });

  socket.on("open", () => {
    ready = true;
    for (const frame of pending.splice(0, pending.length)) sendFrame(frame);
  });
  socket.on("message", (data, isBinary) => {
    const payload = Buffer.isBuffer(data)
      ? data
      : Array.isArray(data)
        ? Buffer.concat(data)
        : Buffer.from(data as ArrayBuffer);
    if (payload.length + 1 > MAX_FRAME_BYTES) {
      stream.close("tunnel frame too large");
      socket.close();
      return;
    }
    stream.send(Buffer.concat([Buffer.from([isBinary ? 1 : 0]), payload]));
  });
  socket.on("close", (code, reason) => {
    stream.close(reason.toString() || `local ws closed ${code}`);
  });
  socket.on("error", (error) => {
    stream.close(error.message);
  });
  stream.onClose(() => {
    if (socket.readyState === WebSocket.OPEN || socket.readyState === WebSocket.CONNECTING) {
      socket.close();
    }
  });
}
