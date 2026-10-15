/**
 * 会话面的剪贴板同步（P1.1，docs/specs/in-app-remote-desktop.md 产品规则 7）。
 *
 * 本模块是**主控侧的门**：下行 `{t:"clip"}` 文本要不要落到本机剪贴板，判据全部来自
 * shared 的回环抑制状态机（同一实例也登记本端写入过的值）。被控侧的门在 host 中继，
 * helper 只负责「变化才发」的流量抑制——一处拥有一份判据，两端不各持一套。
 *
 * 为什么不做主控 → 被控方向：那一半还需要「观测本机剪贴板变化」的读取面（Electron
 * `clipboard.readText` 经 IPC），本模块先把下行方向跑通（P1.1a）；上行方向见计划 P1.1b。
 */
import {
  createRemoteDesktopClipboardSync,
  type RemoteDesktopClipboardSync,
} from "@mode/shared/remote-desktop";

/** 写入本机剪贴板；失败必须 reject（调用方只记日志，不中断会话）。 */
export type RemoteClipboardWriter = (text: string) => Promise<void>;

export interface RemoteDesktopClipboardApplier {
  /** `rd-media-control` 事件入口：只认 `{t:"clip",text}`，其余原样忽略。 */
  onControlMessage(detail: unknown): void;
  /**
   * 轮询读到的本机剪贴板文本（P1.1b）：返回 true = 应当上行给被控端。
   * 同值/空文本/超限/已关闭都返回 false。
   */
  observeLocalText(text: string | null): boolean;
  /** 会话面开关：本端立即生效（开关同时上行给被控中继，两侧一致）。 */
  setEnabled(enabled: boolean): void;
  isEnabled(): boolean;
}

export interface CreateRemoteDesktopClipboardApplierOptions {
  writeText: RemoteClipboardWriter;
  /** 写入失败的回调（会话面只记日志：剪贴板失败不该让画面/输入受影响）。 */
  onWriteError?: (error: unknown) => void;
  enabledInitially?: boolean;
}

export function createRemoteDesktopClipboardApplier(
  options: CreateRemoteDesktopClipboardApplierOptions,
): RemoteDesktopClipboardApplier {
  const gate: RemoteDesktopClipboardSync = createRemoteDesktopClipboardSync(
    options.enabledInitially ?? true,
  );
  return {
    onControlMessage(detail) {
      if (typeof detail !== "object" || detail === null) return;
      const message = detail as { t?: unknown; text?: unknown };
      if (message.t !== "clip" || typeof message.text !== "string") return;
      if (gate.observeRemote(message.text) !== "apply") return;
      void options.writeText(message.text).catch((error: unknown) => {
        options.onWriteError?.(error);
      });
    },
    observeLocalText(text) {
      return gate.observeLocal(text) === "push";
    },
    setEnabled(enabled) {
      gate.setEnabled(enabled);
    },
    isEnabled() {
      return gate.isEnabled();
    },
  };
}
