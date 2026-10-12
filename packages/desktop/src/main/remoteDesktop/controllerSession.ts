/* eslint-disable max-lines -- 主控会话集中维护票据请求、WS 信令转发与状态推送，拆散会让收口路径分散。 */
/**
 * 主控方向会话（main 侧，docs/specs/lan-remote-desktop.md 事件顺序 ③ 与「主控会话状态机」）。
 *
 *   renderer ──rd/start-session──► main（本模块）──host(持令牌代发)──► 对端 POST /api/remote-desktop/session
 *   renderer ◄─rd/session-event─── main ◄──WS ws://对端/ws/remote-desktop?ticket──► 对端 host
 *   renderer ──rd/send-signal───► main ──WS 写出──► 被控 host（仅转发，不解析 SDP/ICE 语义）
 *
 * 握手顺序（两端一致，实现按此对接，改序须同时改被控捕获页注释）：
 *   WS 建立 → 主控 renderer 发 hello{protocolVersion:1} → 被控回 monitors → 主控发 select-monitor
 *   → 被控起采集回 offer → 主控回 answer → 双向 ice trickle（任一端 bye 即结束）。
 *
 * 媒体与输入不经 main：主控 renderer 与对端隐藏渲染器之间 P2P（WebRTC）。
 * 单主控单被控：同时只允许一个控制端会话（第二个 start → busy）。
 */
import { WebSocket } from "ws";
import type { WebContents } from "electron";
import {
  remoteDesktopSignalMessageSchema,
  type RemoteDesktopErrorCode,
  type RemoteDesktopSignalMessage,
} from "@mode/shared/remote-desktop";
import { REMOTE_DESKTOP_IPC } from "./ipcChannels.js";
import { requestRemoteDesktopPeerSessionOpen } from "./hostBridge.js";
import { REMOTE_DESKTOP_SIGNAL_MAX_FRAME_BYTES } from "./hostMessages.js";

export type RemoteDesktopControllerState = "connecting" | "connected" | "closed";

export type RemoteDesktopControllerEvent =
  | { kind: "state"; state: RemoteDesktopControllerState; error?: RemoteDesktopErrorCode }
  | { kind: "signal"; message: RemoteDesktopSignalMessage };

export interface RemoteDesktopControllerSession {
  start(
    serverId: string,
    sender: WebContents,
  ): Promise<{ ok: true } | { error: RemoteDesktopErrorCode }>;
  sendSignal(message: RemoteDesktopSignalMessage): void;
  end(reason?: "user-end" | "error"): Promise<void>;
  dispose(): void;
}

/** 对端错误码兜底：拿不到稳定码时归 internal（不自动重试，由用户手动重连）。 */
function normalizeErrorCode(code: string | undefined): RemoteDesktopErrorCode {
  const known = [
    "consent_denied",
    "consent_timeout",
    "disabled",
    "busy",
    "unsupported",
    "unauthorized",
    "stale_ticket",
    "internal",
  ];
  return known.includes(code ?? "") ? (code as RemoteDesktopErrorCode) : "internal";
}

export function createRemoteDesktopControllerSession(options: {
  logger: { info(...args: unknown[]): void; warn(...args: unknown[]): void };
}): RemoteDesktopControllerSession {
  let socket: WebSocket | null = null;
  let senderWebContents: WebContents | null = null;
  let detachSenderListener: (() => void) | null = null;
  let closedPushed = false;
  let starting = false;
  /** start 在途时到来 end/dispose：结果作废，不再建立 socket（终态已推）。 */
  let startAborted = false;

  const push = (event: RemoteDesktopControllerEvent): void => {
    const target = senderWebContents;
    if (!target || target.isDestroyed()) {
      return;
    }
    target.send(REMOTE_DESKTOP_IPC.SessionEvent, event);
  };

  const pushState = (state: RemoteDesktopControllerState, error?: RemoteDesktopErrorCode): void => {
    if (state === "closed") {
      if (closedPushed) {
        return;
      }
      closedPushed = true;
    }
    push(error ? { kind: "state", state, error } : { kind: "state", state });
  };

  const detachSender = (): void => {
    detachSenderListener?.();
    detachSenderListener = null;
  };

  /** 收口唯一出口（幂等）：关 WS、推 closed、摘发起窗口监听。 */
  const teardown = (error?: RemoteDesktopErrorCode): void => {
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
    pushState("closed", error);
    detachSender();
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
      push({ kind: "signal", message: message.data });
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
      teardown();
    });
    ws.on("error", (error: Error) => {
      if (socket !== ws) {
        return;
      }
      options.logger.warn(`[remote-desktop] controller signal socket error: ${error.message}`);
      teardown("internal");
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
      detachSender();
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
    async start(serverId, sender): Promise<{ ok: true } | { error: RemoteDesktopErrorCode }> {
      if (socket || starting) {
        return { error: "busy" };
      }
      senderWebContents = sender;
      detachSender();
      // 发起窗口关闭：不留半开会话（媒体面随窗口销毁，main 只负责信令收口）。
      const onSenderDestroyed = (): void => {
        void doEnd("user-end");
      };
      sender.once("destroyed", onSenderDestroyed);
      detachSenderListener = () => sender.removeListener("destroyed", onSenderDestroyed);

      starting = true;
      startAborted = false;
      closedPushed = false;
      pushState("connecting");
      let result: Awaited<ReturnType<typeof requestRemoteDesktopPeerSessionOpen>>;
      try {
        result = await requestRemoteDesktopPeerSessionOpen(serverId);
      } catch (error) {
        // 宿主桥 reject（90s 超时 / 无宿主 / 宿主退出）不是稳定错误码：按 internal 收口。
        // 不能把异常抛穿 ipcMain.handle——否则发起窗口的 destroyed 监听泄漏、终态不推。
        options.logger.warn(
          `[remote-desktop] peer session request rejected (serverId=${serverId}): ${
            error instanceof Error ? error.message : String(error)
          }`,
        );
        if (startAborted) {
          senderWebContents = null;
          detachSender();
          return { ok: true };
        }
        pushState("closed", "internal");
        senderWebContents = null;
        detachSender();
        return { error: "internal" };
      } finally {
        starting = false;
      }
      if (startAborted) {
        // 在途期间发起窗口已关闭/被显式 end：终态已推，不再建 socket。
        senderWebContents = null;
        detachSender();
        return { ok: true };
      }
      if (!result.ok) {
        const code = normalizeErrorCode(result.errorCode);
        options.logger.warn(
          `[remote-desktop] peer session request failed (serverId=${serverId}): ${result.error}`,
        );
        senderWebContents = null;
        pushState("closed", code);
        detachSender();
        return { error: code };
      }
      let ws: WebSocket;
      try {
        ws = new WebSocket(result.wsUrl);
      } catch (error) {
        options.logger.warn("[remote-desktop] invalid peer ws url:", error);
        pushState("closed", "internal");
        detachSender();
        senderWebContents = null;
        return { error: "internal" };
      }
      socket = ws;
      attach(ws);
      options.logger.info(`[remote-desktop] controller session requested (serverId=${serverId})`);
      return { ok: true };
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
        // 本地结束：bye 已带出，随后关闭本端 WS 并推终态。
        teardown();
      }
    },
    end: (reason = "user-end") => doEnd(reason),
    dispose(): void {
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
      detachSender();
      senderWebContents = null;
    },
  };
}
