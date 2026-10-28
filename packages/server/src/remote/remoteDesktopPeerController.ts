/* eslint-disable max-lines -- 信令/媒体中继与对端 WS 生命周期共享同一收口语义，拆开易产生第二份 cleanup。 */
/**
 * Web/fpk 远程桌面主控中继（docs/specs/web-remote-desktop.md）。
 *
 * 服务端用凭据里的对端令牌请求会话并直连对端的信令/媒体 WS；浏览器只连 NAS 上的
 * `/ws/remote-desktop-peer/:id/{signal,media}`。服务端不解析媒体帧、不解密：
 * 信令按 schema 校验后转发，媒体按字节双向转发。
 */
import { randomUUID } from "node:crypto";
import { WebSocket, type RawData } from "ws";
import { formatLogPrefix } from "@mode/shared";
import {
  remoteDesktopSignalMessageSchema,
  type RemoteDesktopCodec,
  type RemoteDesktopErrorCode,
  type RemoteDesktopQuality,
  type RemoteDesktopSignalMessage,
} from "@mode/shared/remote-desktop";
import type { ICredentialService } from "@mode/services";
import {
  openRemoteDesktopPeerSession,
  probeRemoteDesktopPeer,
  type RemoteDesktopPeerInfo,
} from "./remoteDesktopPeerClient.js";

const LOG_PREFIX = formatLogPrefix("mode-server:rd-peer", process.pid);
const PEER_SIGNAL_OPEN_TIMEOUT_MS = 10_000;
const MEDIA_IDLE_CLOSE_MS = 60_000;

type BrowserSignalEvent =
  | {
      kind: "state";
      state: "connected" | "closed";
      error?: RemoteDesktopErrorCode;
      retryable?: boolean;
    }
  | { kind: "signal"; message: RemoteDesktopSignalMessage };

interface PeerSession {
  id: string;
  serverId: string;
  peerSignal: WebSocket | null;
  peerMedia: WebSocket | null;
  peerMediaUrl: string;
  browserSignal: WebSocket | null;
  browserMedia: WebSocket | null;
  pendingToPeer: string[];
  /** 对端在浏览器信令 WS attach 之前就发来的信令消息；attach 后按序补发。 */
  pendingToBrowser: BrowserSignalEvent[];
  /** 浏览器媒体 WS 常在 peer 媒体 WS 连上之前就发 `{t:"ready"}`；先排队，open 后按序补发。 */
  pendingMediaToPeer: string[];
  mediaIdleTimer: ReturnType<typeof setTimeout> | null;
  unclaimedTimer: ReturnType<typeof setTimeout> | null;
  closed: boolean;
}

export interface RemoteDesktopPeerSessionStart {
  ok: true;
  sessionId: string;
  signalPath: string;
  mediaPath: string;
}

export type RemoteDesktopPeerSessionStartResult =
  | RemoteDesktopPeerSessionStart
  | { ok: false; errorCode?: RemoteDesktopErrorCode; error: string };

function sendJson(socket: WebSocket | null, payload: BrowserSignalEvent): void {
  if (!socket || socket.readyState !== WebSocket.OPEN) {
    return;
  }
  try {
    socket.send(JSON.stringify(payload));
  } catch {
    // 浏览器 WS 可能已经在关闭中；状态由 close handler 收口。
  }
}

function rawToString(data: RawData): string {
  if (Buffer.isBuffer(data)) {
    return data.toString("utf8");
  }
  if (Array.isArray(data)) {
    return Buffer.concat(data).toString("utf8");
  }
  return Buffer.from(data as ArrayBuffer).toString("utf8");
}

export class RemoteDesktopPeerController {
  private readonly sessions = new Map<string, PeerSession>();

  constructor(
    private readonly credentials: ICredentialService,
    private readonly log: (...args: unknown[]) => void = (...args) =>
      console.log(LOG_PREFIX, ...args),
    private readonly openPeerSession: typeof openRemoteDesktopPeerSession = openRemoteDesktopPeerSession,
    private readonly probePeer: typeof probeRemoteDesktopPeer = probeRemoteDesktopPeer,
  ) {}

  probe(serverId: string): Promise<RemoteDesktopPeerInfo> {
    return this.probePeer(this.credentials, serverId);
  }

  async start(
    serverId: string,
    quality?: RemoteDesktopQuality,
    codec?: RemoteDesktopCodec,
  ): Promise<RemoteDesktopPeerSessionStartResult> {
    const result = await this.openPeerSession(this.credentials, serverId, quality, codec);
    if (!result.ok) {
      return {
        ok: false,
        ...(result.errorCode ? { errorCode: result.errorCode } : {}),
        error: result.error,
      };
    }

    const sessionId = randomUUID();
    const session: PeerSession = {
      id: sessionId,
      serverId,
      peerSignal: null,
      peerMedia: null,
      peerMediaUrl: result.mediaUrl,
      browserSignal: null,
      browserMedia: null,
      pendingToPeer: [],
      pendingToBrowser: [],
      pendingMediaToPeer: [],
      mediaIdleTimer: null,
      unclaimedTimer: null,
      closed: false,
    };
    this.sessions.set(sessionId, session);
    const unclaimedTimer = setTimeout(() => {
      this.closeSession(sessionId, { notifyBrowser: false });
    }, 60_000);
    unclaimedTimer.unref();
    session.unclaimedTimer = unclaimedTimer;

    try {
      const peerSignal = new WebSocket(result.wsUrl);
      session.peerSignal = peerSignal;
      peerSignal.on("message", (data, isBinary) => {
        if (isBinary) {
          return;
        }
        const text = rawToString(data);
        let parsed: unknown;
        try {
          parsed = JSON.parse(text);
        } catch {
          return;
        }
        const message = remoteDesktopSignalMessageSchema.safeParse(parsed);
        if (!message.success) {
          return;
        }
        this.onPeerSignal(session, message.data);
      });
      peerSignal.on("close", () => {
        this.closeSession(sessionId, {
          notifyBrowser: true,
          retryable: true,
          error: "internal",
        });
      });
      peerSignal.on("error", () => {
        // close 事件随后到；这里不重复收口。
      });
      await new Promise<void>((resolve, reject) => {
        const timer = setTimeout(
          () => reject(new Error("peer signaling open timeout")),
          PEER_SIGNAL_OPEN_TIMEOUT_MS,
        );
        timer.unref();
        peerSignal.once("open", () => {
          clearTimeout(timer);
          resolve();
        });
        peerSignal.once("error", (error) => {
          clearTimeout(timer);
          reject(error);
        });
      });
    } catch (error) {
      this.closeSession(sessionId, { notifyBrowser: false });
      return {
        ok: false,
        errorCode: "internal",
        error: `连接对端信令失败：${error instanceof Error ? error.message : String(error)}`,
      };
    }

    return {
      ok: true,
      sessionId,
      signalPath: `/ws/remote-desktop-peer/${sessionId}/signal`,
      mediaPath: `/ws/remote-desktop-peer/${sessionId}/media`,
    };
  }

  /** 浏览器信令 WS 接管：浏览器→对端校验转发；对端→浏览器已有 peerSignal handler。 */
  attachBrowserSignal(sessionId: string, browser: WebSocket): void {
    const session = this.sessions.get(sessionId);
    if (!session || session.closed) {
      try {
        browser.close(4004, "Remote desktop session not found");
      } catch {
        // ignore
      }
      return;
    }
    session.browserSignal = browser;
    if (session.unclaimedTimer) {
      clearTimeout(session.unclaimedTimer);
      session.unclaimedTimer = null;
    }
    sendJson(browser, { kind: "state", state: "connected" });
    const pending = session.pendingToBrowser.splice(0);
    for (const event of pending) {
      sendJson(browser, event);
    }
    browser.on("message", (data, isBinary) => {
      if (isBinary) {
        return;
      }
      let parsed: unknown;
      try {
        parsed = JSON.parse(rawToString(data));
      } catch {
        return;
      }
      if (!parsed || typeof parsed !== "object" || (parsed as { kind?: unknown }).kind !== "signal") {
        return;
      }
      const message = remoteDesktopSignalMessageSchema.safeParse(
        (parsed as { message?: unknown }).message,
      );
      if (!message.success) {
        return;
      }
      this.sendToPeer(session, JSON.stringify(message.data));
    });
    browser.on("close", () => {
      if (session.browserSignal === browser) {
        session.browserSignal = null;
      }
      this.closeSession(sessionId, { notifyBrowser: false });
    });
  }

  /** 浏览器媒体 WS 接管：对端媒体 WS 保持跨浏览器重连复用；60s 无浏览器再回收。 */
  attachBrowserMedia(sessionId: string, browser: WebSocket): void {
    const session = this.sessions.get(sessionId);
    if (!session || session.closed) {
      try {
        browser.close(4004, "Remote desktop session not found");
      } catch {
        // ignore
      }
      return;
    }
    if (session.mediaIdleTimer) {
      clearTimeout(session.mediaIdleTimer);
      session.mediaIdleTimer = null;
    }
    session.browserMedia = browser;

    if (!session.peerMedia) {
      this.openPeerMedia(session, browser);
    } else {
      this.bindPeerMediaToBrowser(session, session.peerMedia, browser);
    }
  }

  end(sessionId: string, reason: "user-end" | "error" = "user-end"): void {
    const session = this.sessions.get(sessionId);
    if (!session) {
      return;
    }
    this.sendToPeer(session, JSON.stringify({ type: "bye", reason }));
    this.closeSession(sessionId, { notifyBrowser: false });
  }

  async disposeAll(): Promise<void> {
    for (const id of this.sessions.keys()) {
      this.closeSession(id, { notifyBrowser: false });
    }
  }

  private openPeerMedia(session: PeerSession, browser: WebSocket): void {
    const peerMedia = new WebSocket(session.peerMediaUrl);
    session.peerMedia = peerMedia;
    peerMedia.on("open", () => {
      const queued = session.pendingMediaToPeer.splice(0);
      for (const text of queued) {
        try {
          peerMedia.send(text);
        } catch {
          session.pendingMediaToPeer.unshift(text);
          break;
        }
      }
    });
    peerMedia.on("message", (data, isBinary) => {
      const target = session.browserMedia;
      if (!target || target.readyState !== WebSocket.OPEN) {
        return;
      }
      try {
        target.send(data, { binary: isBinary });
      } catch {
        // 浏览器 WS 关闭中；close handler 收口。
      }
    });
    peerMedia.on("close", () => {
      if (session.peerMedia === peerMedia) {
        session.peerMedia = null;
      }
      const target = session.browserMedia;
      if (target) {
        try {
          target.close(4001, "Peer media closed");
        } catch {
          // ignore
        }
      }
    });
    peerMedia.on("error", () => {
      // close 事件随后到。
    });
    this.bindPeerMediaToBrowser(session, peerMedia, browser);
  }

  private bindPeerMediaToBrowser(session: PeerSession, peerMedia: WebSocket, browser: WebSocket): void {
    browser.on("message", (data, isBinary) => {
      if (isBinary) {
        return;
      }
      if (peerMedia.readyState !== WebSocket.OPEN) {
        // ready / 首批输入必须在 peer 媒体面 open 后按序补发，不能静默丢。
        if (session.pendingMediaToPeer.length < 64) {
          session.pendingMediaToPeer.push(rawToString(data));
        }
        return;
      }
      try {
        peerMedia.send(data, { binary: false });
      } catch {
        // peer 关闭中；close handler 收口。
      }
    });
    browser.on("close", () => {
      if (session.browserMedia === browser) {
        session.browserMedia = null;
      }
      // 媒体重连时新浏览器 WS 常先于旧 WS 的 close 事件接管；此时不能按「无浏览器」
      // 给对端媒体排 60s 回收，否则会把正在使用的新浏览器一起断掉。
      if (session.closed || session.browserMedia || session.mediaIdleTimer) {
        return;
      }
      session.mediaIdleTimer = setTimeout(() => {
        session.mediaIdleTimer = null;
        const current = session.peerMedia;
        session.peerMedia = null;
        if (current) {
          try {
            current.close();
          } catch {
            // ignore
          }
        }
      }, MEDIA_IDLE_CLOSE_MS);
      session.mediaIdleTimer.unref();
    });
  }

  private onPeerSignal(session: PeerSession, message: RemoteDesktopSignalMessage): void {
    if (message.type === "media-ticket") {
      // 浏览器媒体重连会沿用同一中继路径；若对端重签票据，更新下次打开对端媒体用的地址。
      try {
        const url = new URL(session.peerMediaUrl);
        url.searchParams.set("mediaTicket", message.mediaTicket);
        session.peerMediaUrl = url.toString();
      } catch {
        // 保持旧地址；重连仍复用已打开的对端媒体。
      }
    }
    const event: BrowserSignalEvent = { kind: "signal", message };
    if (session.browserSignal) {
      sendJson(session.browserSignal, event);
    } else if (session.pendingToBrowser.length < 100) {
      session.pendingToBrowser.push(event);
    }
    if (message.type === "bye" || message.type === "error") {
      this.closeSession(session.id, {
        notifyBrowser: true,
        retryable: false,
        ...(message.type === "error" ? { error: message.code } : {}),
      });
    }
  }

  private sendToPeer(session: PeerSession, text: string): void {
    const peer = session.peerSignal;
    if (!peer || peer.readyState !== WebSocket.OPEN) {
      session.pendingToPeer.push(text);
      return;
    }
    try {
      peer.send(text);
    } catch {
      session.pendingToPeer.push(text);
    }
  }

  private closeSession(
    sessionId: string,
    options: {
      notifyBrowser: boolean;
      retryable?: boolean;
      error?: RemoteDesktopErrorCode;
    },
  ): void {
    const session = this.sessions.get(sessionId);
    if (!session || session.closed) {
      return;
    }
    session.closed = true;
    this.sessions.delete(sessionId);
    if (session.mediaIdleTimer) {
      clearTimeout(session.mediaIdleTimer);
      session.mediaIdleTimer = null;
    }
    if (session.unclaimedTimer) {
      clearTimeout(session.unclaimedTimer);
      session.unclaimedTimer = null;
    }
    if (options.notifyBrowser && session.browserSignal) {
      sendJson(session.browserSignal, {
        kind: "state",
        state: "closed",
        retryable: options.retryable === true,
        ...(options.error ? { error: options.error } : {}),
      });
    }
    for (const socket of [session.browserSignal, session.browserMedia, session.peerMedia, session.peerSignal]) {
      if (!socket) {
        continue;
      }
      try {
        socket.close();
      } catch {
        // ignore
      }
    }
    session.pendingMediaToPeer.length = 0;
    session.pendingToPeer.length = 0;
    session.pendingToBrowser.length = 0;
    session.browserSignal = null;
    session.browserMedia = null;
    session.peerMedia = null;
    session.peerSignal = null;
  }
}
