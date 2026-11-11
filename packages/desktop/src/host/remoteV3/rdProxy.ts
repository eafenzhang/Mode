import { randomUUID } from "node:crypto";
import { WebSocketServer, WebSocket } from "ws";

import type { RemoteStream } from "@mode/remote-core";

type ProxyKind = "signal" | "media";

interface ProxySession {
  signal: RemoteStream;
  /** 媒体流可因瞬断重开；信令存活期间为 null 表示等待 renderer 重连。 */
  media: RemoteStream | null;
  sockets: { signal: WebSocket | null; media: WebSocket | null };
  reopenMedia?: (mediaTicket: string) => Promise<RemoteStream>;
  lastMediaTicket?: string;
  mediaOpen: Promise<void> | null;
  closed: boolean;
}

/**
 * 把 v3 隧道桥接成本机 `ws://127.0.0.1:<port>`，让现有远程桌面 controller 与
 * renderer 媒体客户端保持原样。代理只监听回环，不暴露到局域网。
 *
 * 媒体隧道瞬断时保留 signal 会话与同一回环 URL；renderer 用新的 `mediaTicket`
 * 重连时，代理按 ticket 重开 `remote.ws` 媒体流，实现隧道级媒体重连。
 */
export class RemoteDesktopTunnelProxy {
  private wss: WebSocketServer | null = null;
  private port = 0;
  private starting: Promise<void> | null = null;
  private readonly sessions = new Map<string, ProxySession>();

  async start(): Promise<number> {
    if (this.port) return this.port;
    if (this.starting) {
      await this.starting;
      return this.port;
    }
    this.starting = new Promise<void>((resolve, reject) => {
      const wss = new WebSocketServer({ host: "127.0.0.1", port: 0 });
      wss.once("listening", () => {
        const address = wss.address();
        if (typeof address === "object" && address) this.port = address.port;
        resolve();
      });
      wss.once("error", reject);
      wss.on("connection", (socket, req) => this.attach(socket, req.url ?? ""));
      this.wss = wss;
    });
    try {
      await this.starting;
    } finally {
      this.starting = null;
    }
    if (!this.port) throw new Error("远程桌面本地代理端口未就绪");
    return this.port;
  }

  async register(
    signal: RemoteStream,
    media: RemoteStream,
    options: { reopenMedia?: (mediaTicket: string) => Promise<RemoteStream> } = {},
  ): Promise<{ signalUrl: string; mediaUrl: string }> {
    const port = await this.start();
    const id = randomUUID();
    const session: ProxySession = {
      signal,
      media,
      sockets: { signal: null, media: null },
      ...(options.reopenMedia ? { reopenMedia: options.reopenMedia } : {}),
      mediaOpen: null,
      closed: false,
    };
    this.sessions.set(id, session);
    this.bindSignal(id, session);
    this.bindMedia(session, media);
    return {
      signalUrl: `ws://127.0.0.1:${port}/rd/${id}/signal`,
      mediaUrl: `ws://127.0.0.1:${port}/rd/${id}/media`,
    };
  }

  close(): void {
    for (const [id, session] of this.sessions) {
      this.sessions.delete(id);
      session.closed = true;
      session.signal.close("proxy closed");
      session.media?.close("proxy closed");
      session.sockets.signal?.close();
      session.sockets.media?.close();
    }
    this.wss?.close();
    this.wss = null;
    this.port = 0;
  }

  private bindSignal(id: string, session: ProxySession): void {
    session.signal.onData((frame) => {
      const binary = (frame[0] ?? 1) === 1;
      const payload = Buffer.from(frame.subarray(1));
      if (!binary) this.observeSignalText(session, payload.toString());
      const socket = session.sockets.signal;
      if (socket?.readyState === WebSocket.OPEN) {
        socket.send(payload, { binary });
      }
    });
    session.signal.onClose(() => {
      session.closed = true;
      session.sockets.signal?.close();
      session.sockets.media?.close();
      session.media?.close("signal closed");
      this.sessions.delete(id);
    });
  }

  /** signal 上的 `media-ticket` 是 renderer 媒体重连要用的新票据。 */
  private observeSignalText(session: ProxySession, text: string): void {
    if (!text.startsWith("{")) return;
    try {
      const frame = JSON.parse(text) as { type?: unknown; mediaTicket?: unknown };
      if (frame.type === "media-ticket" && typeof frame.mediaTicket === "string") {
        session.lastMediaTicket = frame.mediaTicket;
      }
    } catch {
      // 非 JSON 信令帧忽略。
    }
  }

  private bindMedia(session: ProxySession, stream: RemoteStream): void {
    stream.onData((frame) => {
      const socket = session.sockets.media;
      if (socket?.readyState === WebSocket.OPEN) {
        socket.send(Buffer.from(frame.subarray(1)), { binary: (frame[0] ?? 1) === 1 });
      }
    });
    stream.onClose(() => {
      if (session.media !== stream) return;
      session.media = null;
      const socket = session.sockets.media;
      session.sockets.media = null;
      socket?.close();
    });
  }

  private attach(socket: WebSocket, rawUrl: string): void {
    const url = new URL(rawUrl, "http://127.0.0.1");
    const match = /^\/rd\/([^/]+)\/(signal|media)$/.exec(url.pathname);
    if (!match) {
      socket.close(1008, "invalid proxy path");
      return;
    }
    const id = match[1]!;
    const kind = match[2] as ProxyKind;
    const session = this.sessions.get(id);
    if (!session) {
      socket.close(1008, "unknown proxy session");
      return;
    }
    if (session.sockets[kind]) {
      socket.close(1008, "proxy session busy");
      return;
    }
    if (kind === "signal") {
      session.sockets.signal = socket;
      this.attachSocket(socket, session, session.signal);
      return;
    }
    session.sockets.media = socket;
    const ticket = url.searchParams.get("mediaTicket") ?? session.lastMediaTicket;
    if (session.media) {
      this.attachSocket(socket, session, session.media);
      return;
    }
    if (!ticket || !session.reopenMedia || session.mediaOpen) {
      socket.close(1011, "media stream unavailable");
      return;
    }
    const pending: Array<{ data: Buffer; binary: boolean }> = [];
    socket.on("message", (data, isBinary) => {
      const payload = Buffer.isBuffer(data)
        ? data
        : Array.isArray(data)
          ? Buffer.concat(data)
          : Buffer.from(data as ArrayBuffer);
      if (session.media) this.sendFrame(session.media, payload, isBinary);
      else pending.push({ data: payload, binary: isBinary });
    });
    session.mediaOpen = session
      .reopenMedia(ticket)
      .then((stream) => {
        if (session.closed || session.sockets.media !== socket) {
          stream.close("proxy session closed");
          return;
        }
        session.media = stream;
        this.bindMedia(session, stream);
        for (const item of pending.splice(0, pending.length)) {
          this.sendFrame(stream, item.data, item.binary);
        }
      })
      .catch(() => {
        socket.close(1011, "media reopen failed");
      })
      .finally(() => {
        session.mediaOpen = null;
      });
  }

  private attachSocket(socket: WebSocket, session: ProxySession, stream: RemoteStream): void {
    socket.on("message", (data, isBinary) => {
      const payload = Buffer.isBuffer(data)
        ? data
        : Array.isArray(data)
          ? Buffer.concat(data)
          : Buffer.from(data as ArrayBuffer);
      this.sendFrame(stream, payload, isBinary);
    });
    socket.on("close", () => {
      const kind = session.sockets.signal === socket ? "signal" : "media";
      if (session.sockets[kind] === socket) session.sockets[kind] = null;
      if (session.signal === stream || session.media === stream) {
        stream.close("local socket closed");
      }
    });
    socket.on("error", () => {
      if (session.signal === stream || session.media === stream) {
        stream.close("local socket error");
      }
    });
  }

  private sendFrame(stream: RemoteStream, payload: Buffer, binary: boolean): void {
    try {
      stream.send(Buffer.concat([Buffer.from([binary ? 1 : 0]), payload]));
    } catch {
      // 流已关闭：renderer/对端正在收尾，丢弃这一帧即可，不能把异常抛进 ws 回调。
    }
  }
}
