import { WebSocket } from "ws";

import {
  remoteDesktopSignalMessageSchema,
  type RemoteDesktopCodec,
  type RemoteDesktopErrorCode,
  type RemoteDesktopQuality,
  type RemoteDesktopSessionEvent,
  type RemoteDesktopSignalMessage,
  type RemoteDesktopStartSessionResult,
} from "@mode/shared/remote-desktop";

import { REMOTE_DESKTOP_SIGNAL_MAX_FRAME_BYTES } from "./remoteDesktopMessages.js";

/**
 * 主控方向会话（Host 侧，LR-3）。Electron 线的 `main/remoteDesktop/controllerSession.ts`
 * 依赖 `WebContents` 推事件与发起窗口销毁监听；Host 没有窗口，事件改为回调交给 RPC 频道。
 *
 * 状态机与 Electron 线保持一致（单会话互斥、连接/断开、bye/error 收口、retryable 标记）：
 *
 *   renderer ──rdStartSession──► Host controller ──openSession──► 对端 POST + WS
 *   renderer ◄─onDynamicSessionEvent─ Host controller ◄──WS 信令── 对端 host
 *   renderer ──rdSendSignal/rdEndSession──► Host controller ──WS 写出──► 对端 host
 *
 * 媒体与输入不经 Host：画面/输入走主控 renderer 与对端 host 之间的 media WS。
 */

export interface RemoteDesktopPeerSessionOpen {
  ok: true;
  wsUrl: string;
  mediaUrl?: string;
}

export interface RemoteDesktopPeerSessionFailure {
  ok: false;
  errorCode?: string;
  error: string;
}

export interface CreateRemoteDesktopHostControllerOptions {
  /** 打开对端会话（默认实现用已存令牌 POST；测试可注入本地 WS 地址）。 */
  openSession: (
    serverId: string,
    quality?: RemoteDesktopQuality,
    codec?: RemoteDesktopCodec,
  ) => Promise<RemoteDesktopPeerSessionOpen | RemoteDesktopPeerSessionFailure>;
  logger: { info(...args: unknown[]): void; warn(...args: unknown[]): void };
  /** 事件出口；不缓冲，只发给当前订阅者。 */
  push: (event: RemoteDesktopSessionEvent) => void;
}

export interface RemoteDesktopHostController {
  start(params: {
    serverId: string;
    quality?: RemoteDesktopQuality;
    codec?: RemoteDesktopCodec;
  }): Promise<RemoteDesktopStartSessionResult>;
  sendSignal(message: RemoteDesktopSignalMessage): void;
  end(reason?: "user-end" | "error"): Promise<void>;
  /** 退出前带出 bye 并关 WS。 */
  dispose(): Promise<void>;
}

function normalizeErrorCode(code: string | undefined): RemoteDesktopErrorCode {
  const known: RemoteDesktopErrorCode[] = [
    "consent_denied",
    "consent_timeout",
    "disabled",
    "busy",
    "unsupported",
    "unauthorized",
    "stale_ticket",
    "locally_controlled",
    "internal",
  ];
  return known.includes(code as RemoteDesktopErrorCode)
    ? (code as RemoteDesktopErrorCode)
    : "internal";
}

export function createRemoteDesktopHostController(
  options: CreateRemoteDesktopHostControllerOptions,
): RemoteDesktopHostController {
  let socket: WebSocket | null = null;
  let closedPushed = false;
  let starting = false;
  /** start 在途时到来 end/dispose：结果作废，不再建立 socket（终态已推）。 */
  let startAborted = false;

  const pushState = (
    state: "connecting" | "connected" | "closed",
    error?: RemoteDesktopErrorCode,
    retryable?: boolean,
  ): void => {
    if (state === "closed") {
      if (closedPushed) {
        return;
      }
      closedPushed = true;
    }
    options.push(
      error
        ? { kind: "state", state, error, ...(retryable ? { retryable } : {}) }
        : { kind: "state", state, ...(retryable ? { retryable } : {}) },
    );
  };

  /** 收口唯一出口（幂等）：关 WS、推 closed。 */
  const teardown = (error?: RemoteDesktopErrorCode, teardownOptions?: { retryable?: boolean }) => {
    const current = socket;
    socket = null;
    if (current) {
      try {
        current.removeAllListeners();
        current.close();
      } catch {
        // 对端已断开时 close 允许幂等失败。
      }
    }
    pushState("closed", error, teardownOptions?.retryable);
  };

  const attach = (ws: WebSocket): void => {
    ws.on("open", () => {
      if (socket !== ws) {
        return;
      }
      options.logger.info("[remote-desktop] controller signal socket connected");
      pushState("connected");
    });
    ws.on("message", (data: unknown) => {
      if (socket !== ws) {
        return;
      }
      const text =
        typeof data === "string" ? data : Buffer.isBuffer(data) ? data.toString("utf8") : "";
      if (!text) {
        return;
      }
      if (Buffer.byteLength(text, "utf8") > REMOTE_DESKTOP_SIGNAL_MAX_FRAME_BYTES) {
        options.logger.warn("[remote-desktop] controller signal frame over 64KiB, closing session");
        teardown("internal");
        return;
      }
      let parsed: unknown;
      try {
        parsed = JSON.parse(text);
      } catch {
        options.logger.warn("[remote-desktop] dropping malformed signal frame from peer");
        return;
      }
      const message = remoteDesktopSignalMessageSchema.safeParse(parsed);
      if (!message.success) {
        options.logger.warn("[remote-desktop] dropping invalid signal frame from peer");
        return;
      }
      options.push({ kind: "signal", message: message.data });
      if (message.data.type === "bye") {
        teardown();
        return;
      }
      if (message.data.type === "error") {
        teardown(message.data.code);
      }
    });
    ws.on("close", () => {
      if (socket !== ws) {
        return;
      }
      options.logger.info("[remote-desktop] controller signal socket closed");
      // 传输断开（未经 bye / 本地结束）：标记可重连，由 renderer 走会话级重连退避。
      teardown(undefined, { retryable: true });
    });
    ws.on("error", (error: Error) => {
      if (socket !== ws) {
        return;
      }
      options.logger.warn(`[remote-desktop] controller signal socket error: ${error.message}`);
      teardown("internal", { retryable: true });
    });
  };

  const doEnd = async (reason: "user-end" | "error"): Promise<void> => {
    if (starting) {
      // start 在途：标记作废，等它回来后自行收尾（此时还没有 socket 可发 bye）。
      startAborted = true;
      return;
    }
    const current = socket;
    if (!current) {
      pushState("closed");
      return;
    }
    if (current.readyState === WebSocket.OPEN) {
      try {
        current.send(JSON.stringify({ type: "bye", reason }));
      } catch {
        // 通道已坏时直接进入收尾。
      }
    }
    teardown();
  };

  return {
    async start(params): Promise<RemoteDesktopStartSessionResult> {
      if (socket || starting) {
        return { error: "busy" };
      }
      starting = true;
      startAborted = false;
      closedPushed = false;
      pushState("connecting");
      let result: RemoteDesktopPeerSessionOpen | RemoteDesktopPeerSessionFailure;
      try {
        result = await options.openSession(params.serverId, params.quality, params.codec);
      } catch (error) {
        options.logger.warn(
          `[remote-desktop] peer session request rejected (serverId=${params.serverId}): ${
            error instanceof Error ? error.message : String(error)
          }`,
        );
        starting = false;
        if (startAborted) {
          return { ok: true };
        }
        pushState("closed", "internal");
        return { error: "internal" };
      }
      starting = false;
      if (startAborted) {
        // 在途期间被显式 end/dispose：终态已推，不再建 socket。
        return { ok: true };
      }
      if (!result.ok) {
        const code = normalizeErrorCode(result.errorCode);
        options.logger.warn(
          `[remote-desktop] peer session request failed (serverId=${params.serverId}): ${result.error}`,
        );
        pushState("closed", code);
        return { error: code };
      }
      let ws: WebSocket;
      try {
        ws = new WebSocket(result.wsUrl);
      } catch (error) {
        options.logger.warn("[remote-desktop] invalid peer ws url:", error);
        pushState("closed", "internal");
        return { error: "internal" };
      }
      socket = ws;
      attach(ws);
      options.logger.info(
        `[remote-desktop] controller session requested (serverId=${params.serverId})`,
      );
      return { ok: true, ...(result.mediaUrl ? { mediaUrl: result.mediaUrl } : {}) };
    },

    sendSignal(message: RemoteDesktopSignalMessage): void {
      const parsed = remoteDesktopSignalMessageSchema.safeParse(message);
      if (!parsed.success) {
        options.logger.warn("[remote-desktop] dropping invalid outgoing signal");
        return;
      }
      const ws = socket;
      if (!ws || ws.readyState !== WebSocket.OPEN) {
        return;
      }
      const frame = JSON.stringify(parsed.data);
      if (Buffer.byteLength(frame, "utf8") > REMOTE_DESKTOP_SIGNAL_MAX_FRAME_BYTES) {
        options.logger.warn("[remote-desktop] outgoing signal over 64KiB, dropping frame");
        return;
      }
      try {
        ws.send(frame);
      } catch (error) {
        options.logger.warn(
          "[remote-desktop] signal send failed:",
          error instanceof Error ? error.message : error,
        );
        return;
      }
      if (parsed.data.type === "bye") {
        teardown();
      }
    },

    end: (reason = "user-end") => doEnd(reason),

    async dispose(): Promise<void> {
      const wasStarting = starting;
      if (!wasStarting) {
        await doEnd("user-end");
        return;
      }
      // start 在途：标记作废；正在等待的 openSession 返回后不会再建 socket。
      startAborted = true;
      const current = socket;
      socket = null;
      if (current) {
        try {
          current.removeAllListeners();
          current.close();
        } catch {
          // 幂等失败忽略。
        }
      }
    },
  };
}
