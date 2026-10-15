/**
 * 被控侧剪贴板文本的门 + 出站（P1.1，docs/specs/in-app-remote-desktop.md 产品规则 7）。
 *
 * 分工：helper 媒体泵只做两件「哑」事——本端文本**变了**就上报（kind5）、收到 kind6 就把主控
 * 文本落到本端剪贴板；**采纳不采纳在这里定**。判据来自 shared 的回环抑制状态机：已经同步过的值
 * （无论本端刚写进去的还是刚收到并应用的）再被观测到就不算变化。
 *
 * 为什么门在被控 host 而不在 helper：helper 是 allowlist 内的纯 .mjs，无法复用 shared 的
 * TypeScript 实现；复制一份判据必然漂移，所以判据只留一处。出站发送也收在这里，中继因此
 * 只保留「按 kind 分发字节」的字节泵职责（max-lines 约束，同 RemoteControlPairedClientsList 先例）。
 */
import {
  createRemoteDesktopClipboardSync,
  remoteDesktopClipboardEventSchema,
  remoteDesktopClipboardModeSchema,
  type RemoteDesktopClipboardSync,
} from "@mode/shared/remote-desktop";

export interface RemoteDesktopClipboardGateOptions {
  /** 当前 media WS（未挂接返回 null）：采纳的文本经它下行 `{t:"clip"}`。 */
  getWebSocket: () => { send(data: string, opts?: unknown): void } | null;
  logger: { warn(...args: unknown[]): void };
}

export interface RemoteDesktopClipboardGate {
  /** helper→host kind5（被控端剪贴板变化）：采纳则立刻下行；重复值/已关闭/超限静默丢弃。 */
  handleHelperText(text: string): void;
  /**
   * media WS 上行 `{t:"clip", text}`（主控剪贴板 → 被控，P1.1b）：载荷合法即返回 true，
   * 采纳时经 `write` 落到被控剪贴板；重复值/已关闭/超限不写（静默）。
   */
  handleControllerText(message: unknown, write: (text: string) => void): boolean;
  /** media WS 上行 `{t:"clip-mode"}`：应用开关；返回 false = 载荷非法（调用方记 warn）。 */
  handleModeMessage(message: unknown): boolean;
  isEnabled(): boolean;
}

export function createRemoteDesktopClipboardGate(
  options: RemoteDesktopClipboardGateOptions,
): RemoteDesktopClipboardGate {
  const sync: RemoteDesktopClipboardSync = createRemoteDesktopClipboardSync();
  return {
    handleHelperText(text) {
      // 被控端**本端**剪贴板变了：observeLocal 语义（登记后同值不再上行）。
      if (sync.observeLocal(text) !== "push") {
        return;
      }
      const ws = options.getWebSocket();
      if (!ws) {
        return; // WS 未挂接：无人可送（会话尚未就绪），下一次变化会再来。
      }
      try {
        ws.send(JSON.stringify({ t: "clip", text }));
      } catch (error) {
        options.logger.warn("[rd-media-relay] clipboard send failed:", error);
      }
    },
    handleControllerText(message, write) {
      const parsed = remoteDesktopClipboardEventSchema.safeParse(message);
      if (!parsed.success) {
        return false;
      }
      // 对端文本要落到被控端：observeRemote 语义（写进去的值会被本端轮询读到，不得回弹）。
      if (sync.observeRemote(parsed.data.text) === "apply") {
        try {
          write(parsed.data.text);
        } catch (error) {
          options.logger.warn("[rd-media-relay] clipboard write to helper failed:", error);
        }
      }
      return true;
    },
    handleModeMessage(message) {
      const parsed = remoteDesktopClipboardModeSchema.safeParse(message);
      if (!parsed.success) {
        return false;
      }
      sync.setEnabled(parsed.data.enabled);
      return true;
    },
    isEnabled() {
      return sync.isEnabled();
    },
  };
}
