/**
 * 被控侧剪贴板文本的门 + 出站（P1.1，docs/specs/in-app-remote-desktop.md 产品规则 7）。
 *
 * 分工：helper 媒体泵只做两件「哑」事——本端文本**变了**就上报（kind5）、收到 kind6 就把主控
 * 文本落到本端剪贴板；**采纳不采纳在这里定**。判据来自 shared 的回环抑制状态机：已经同步过的值
 * （无论本端刚写进去的还是刚收到并应用的）再被观测到就不算变化。
 *
 * 为什么门在被控 host 而不在 helper：helper 是 allowlist 内的纯 .mjs，无法复用 shared 的
 * TypeScript 实现；复制一份判据必然漂移，所以判据只留一处。出站发送（含 helper 未接通时的
 * kind6 留存与补写）也收在这里，中继因此只保留「按 kind 分发字节」的字节泵职责
 *（max-lines 约束，同 RemoteControlPairedClientsList 先例）。
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
  /**
   * 把主控文本写到被控剪贴板（helper 的 kind6），返回是否真写进了管道。
   * helper 未接通时返回 false，门据此**留存**该值、等接通后补写——见下方 pendingText 的说明。
   */
  writeHelper: (text: string) => boolean;
  logger: { info(...args: unknown[]): void; warn(...args: unknown[]): void };
}

export interface RemoteDesktopClipboardGate {
  /** helper→host kind5（被控端剪贴板变化）：采纳则立刻下行；重复值/已关闭/超限静默丢弃。 */
  handleHelperText(text: string): void;
  /**
   * media WS 上行 `{t:"clip", text}`（主控剪贴板 → 被控，P1.1b）：载荷合法即返回 true，
   * 采纳时经 `writeHelper` 落到被控剪贴板；重复值/已关闭/超限不写（静默）。
   */
  handleControllerText(message: unknown): boolean;
  /** media WS 上行 `{t:"clip-mode"}`：应用开关；返回 false = 载荷非法（调用方记 warn）。 */
  handleModeMessage(message: unknown): boolean;
  /** helper 接通后补写留存的主控文本（无留存则无操作）；中继在 helper 连接事件里调用。 */
  flushPendingWrite(): void;
  /** 会话收口：作废留存的主控文本（不得跨会话补写）。 */
  clearPendingWrite(): void;
  isEnabled(): boolean;
}

export function createRemoteDesktopClipboardGate(
  options: RemoteDesktopClipboardGateOptions,
): RemoteDesktopClipboardGate {
  const sync: RemoteDesktopClipboardSync = createRemoteDesktopClipboardSync();
  /**
   * 已采纳、但还没写进 helper 的主控文本（helper 管道尚未接通）。
   *
   * 为什么不能像 kind3/kind4 那样「丢了等下一次变化」：它承载的是**值**而不是可重算的提示。
   * 主控侧门在上行时就登记过这个值，被控侧门（下方 observeRemote）也在写 helper **之前**就把它
   * 登记成 lastSynced——两端都认为「已同步」，所以同一个值再观测到不算变化，不会再有下一次重试。
   * 于是「未接通窗口里丢掉的 kind6」= 一次**永久且静默**的剪贴板同步丢失。而窗口必然出现：
   * 同意后中继与媒体 WS 先就绪，helper 要等 rd_media_start 拉起进程才连上来，主控在窗口里的
   * 第一次上行（通常正是「连上时本机剪贴板里的值」）就落在这里（2026-10-16 定位）。
   * 语义是「最新值获胜」：剪贴板只关心当前值，后到的留存值直接顶掉上一条，不排队。
   */
  let pendingText: string | null = null;

  /** 写 kind6：写进管道即清留存位，否则留存（顶掉上一条）并留痕。 */
  const writeToHelper = (text: string): void => {
    try {
      if (options.writeHelper(text)) {
        pendingText = null;
        return;
      }
      // 留痕：这次同步没有落地（helper 未接通），值留下来等接通补写。修复前这条路径是**静默丢弃**
      // （门已登记 lastSynced，同值不再重发），整条链路因此完全不可观测——所以这里必须留下一行。
      options.logger.info("[rd-media-relay] helper not connected, clipboard text held for flush");
      pendingText = text;
    } catch (error) {
      pendingText = text; // 写失败同样按「未落地」处理，等接通/补写再试。
      options.logger.warn("[rd-media-relay] clipboard write to helper failed:", error);
    }
  };

  /** 作废留存值并留痕：会话收口 / 开关关闭是唯一两条「明确丢弃」的路径。 */
  const discardPending = (reason: string): void => {
    if (pendingText === null) {
      return;
    }
    options.logger.info(`[rd-media-relay] pending clipboard text discarded: ${reason}`);
    pendingText = null;
  };

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
    handleControllerText(message) {
      const parsed = remoteDesktopClipboardEventSchema.safeParse(message);
      if (!parsed.success) {
        return false;
      }
      // 对端文本要落到被控端：observeRemote 语义（写进去的值会被本端轮询读到，不得回弹）。
      // 登记发生在写之前是刻意的：写不进去的值由 pendingText 兜住，不是丢在地上。
      if (sync.observeRemote(parsed.data.text) === "apply") {
        writeToHelper(parsed.data.text);
      }
      return true;
    },
    flushPendingWrite() {
      if (pendingText !== null) {
        writeToHelper(pendingText);
      }
    },
    clearPendingWrite() {
      discardPending("session closed");
    },
    handleModeMessage(message) {
      const parsed = remoteDesktopClipboardModeSchema.safeParse(message);
      if (!parsed.success) {
        return false;
      }
      sync.setEnabled(parsed.data.enabled);
      if (!parsed.data.enabled) {
        // 关掉同步后不该再往被控剪贴板写任何东西（含未接通的留存值）。与「关闭不重置 lastSynced」
        // 不冲突：那条约定管的是重开后不补发关闭期间的旧值，这里做的正是同一件事。
        discardPending("clipboard disabled");
      }
      return true;
    },
    isEnabled() {
      return sync.isEnabled();
    },
  };
}
