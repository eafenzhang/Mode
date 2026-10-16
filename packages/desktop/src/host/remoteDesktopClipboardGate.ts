/**
 * 被控侧剪贴板文本的门 + 出站（P1.1，docs/specs/in-app-remote-desktop.md 产品规则 7）。
 *
 * 分工：helper 媒体泵只做两件「哑」事——本端文本**变了**就上报（kind5）、收到 kind6 就把主控
 * 文本落到本端剪贴板；**采纳不采纳在这里定**。判据来自 shared 的回环抑制状态机：已经同步过的值
 * （无论本端刚写进去的还是刚收到并应用的）再被观测到就不算变化。
 *
 * 为什么门在被控 host 而不在 helper：helper 是 allowlist 内的纯 .mjs，无法复用 shared 的
 * TypeScript 实现；复制一份判据必然漂移，所以判据只留一处。出站发送（含 helper 未接通时的
 * kind6 留存与补写、media WS 未挂接时的 kind5 留存与补上行）也收在这里，中继因此只保留
 * 「按 kind 分发字节」的字节泵职责（max-lines 约束，同 RemoteControlPairedClientsList 先例）。
 * 两个方向的留存形态一致：**最新值获胜、不排队**，接通/挂接时补发，会话收口与关开关时作废。
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
  /** helper→host kind5（被控端剪贴板变化）：采纳即立刻下行（media WS 未挂接则留存、挂接时补上行）；重复值/已关闭/超限静默丢弃。 */
  handleHelperText(text: string): void;
  /**
   * media WS 上行 `{t:"clip", text}`（主控剪贴板 → 被控，P1.1b）：载荷合法即返回 true，
   * 采纳时经 `writeHelper` 落到被控剪贴板；重复值/已关闭/超限不写（静默）。
   */
  handleControllerText(message: unknown): boolean;
  /** media WS 上行 `{t:"clip-mode"}`：应用开关；返回 false = 载荷非法（调用方记 warn）。 */
  handleModeMessage(message: unknown): boolean;
  /** helper 接通后补写留存的主控文本（kind6 挂起位，无留存则无操作）；中继在 helper 连接事件里调用。 */
  flushPendingToHelper(): void;
  /** media WS 挂接后补上行留存的被控文本（kind5 挂起位，无留存则无操作）；中继在 attachWebSocket 里调用。 */
  flushPendingToController(): void;
  /** 会话收口：作废两个方向留存的值（都不得跨会话补发）。 */
  clearPending(): void;
  isEnabled(): boolean;
}

export function createRemoteDesktopClipboardGate(
  options: RemoteDesktopClipboardGateOptions,
): RemoteDesktopClipboardGate {
  const sync: RemoteDesktopClipboardSync = createRemoteDesktopClipboardSync();
  /**
   * kind6 挂起位：已采纳、但还没写进 helper 的主控文本（helper 管道尚未接通）。
   *
   * 为什么不能像 kind3/kind4 那样「丢了等下一次变化」：它承载的是**值**而不是可重算的提示。
   * 主控侧门在上行时就登记过这个值，被控侧门（下方 observeRemote）也在写 helper **之前**就把它
   * 登记成 lastSynced——两端都认为「已同步」，所以同一个值再观测到不算变化，不会再有下一次重试。
   * 于是「未接通窗口里丢掉的 kind6」= 一次**永久且静默**的剪贴板同步丢失。而窗口必然出现：
   * 同意后中继与媒体 WS 先就绪，helper 要等 rd_media_start 拉起进程才连上来，主控在窗口里的
   * 第一次上行（通常正是「连上时本机剪贴板里的值」）就落在这里（2026-10-16 定位）。
   * 语义是「最新值获胜」：剪贴板只关心当前值，后到的留存值直接顶掉上一条，不排队。
   */
  let pendingToHelper: string | null = null;
  /**
   * kind5 挂起位：已采纳、但还没上行给主控的被控文本（media WS 未挂接）。
   *
   * 与 kind6 **同形**，因为失败形态与补救完全一致，而且这里的登记同样发生在发送**之前**：
   * observeLocal 先把值登记成 lastSynced（同值不再算变化），helper 侧的 pollClipboard 也已推进
   * 自己的 lastClipboard（mediaPump.mjs）——两端都认为「已同步」，这个值不会再有第二次机会。
   * 窗口在真实产品里可达：媒体 WS 断开时中继只清 currentWs（attachWebSocket 的 close 订阅），
   * **helper 管道不受影响**（rd_media_stop 只在会话收口/控制模式里发），helper 仍在 700ms 轮询；
   * 主控按 250ms~2s 退避重连并重新挂接（nativeMediaSession 的 onRequestMediaUrl → host
   * onMediaSocket → 中继 attachWebSocket）。于是断连期被控端复制的值在重连后既不会被 helper 重报
   * （它的 lastClipboard 已推进），也不会被门重判为变化——推流恢复了，只有这一条剪贴板同步永久、
   * 静默地丢失（2026-10-16 定位）。语义同样是「最新值获胜」，不排队。
   */
  let pendingToController: string | null = null;

  /** 写 kind6：写进管道即清留存位，否则留存（顶掉上一条）并留痕。 */
  const writeToHelper = (text: string): void => {
    try {
      if (options.writeHelper(text)) {
        pendingToHelper = null;
        return;
      }
      // 留痕：这次同步没有落地（helper 未接通），值留下来等接通补写。修复前这条路径是**静默丢弃**
      // （门已登记 lastSynced，同值不再重发），整条链路因此完全不可观测——所以这里必须留下一行。
      options.logger.info("[rd-media-relay] helper not connected, clipboard text held for flush");
      pendingToHelper = text;
    } catch (error) {
      pendingToHelper = text; // 写失败同样按「未落地」处理，等接通/补写再试。
      options.logger.warn("[rd-media-relay] clipboard write to helper failed:", error);
    }
  };

  /** 上行 kind5：送进 media WS 即清留存位，否则留存（顶掉上一条）并留痕——同 kind6 的形态。 */
  const pushToController = (text: string): void => {
    const ws = options.getWebSocket();
    if (!ws) {
      // 留痕：这次同步没有上行（媒体 WS 未挂接）。修复前这里同样**静默**（直接 return，
      // 而门已登记 lastSynced），所以窗口内的一次同步丢失在两端都没有任何痕迹。
      options.logger.info("[rd-media-relay] media ws not attached, clipboard text held for flush");
      pendingToController = text;
      return;
    }
    try {
      ws.send(JSON.stringify({ t: "clip", text }));
      pendingToController = null;
    } catch (error) {
      pendingToController = text; // 发送失败同样按「未上行」处理，重挂接后再试。
      options.logger.warn("[rd-media-relay] clipboard send failed:", error);
    }
  };

  /** 作废留存值并留痕：会话收口 / 开关关闭是仅有的两条「明确丢弃」路径（两个方向都作废）。 */
  const discardPending = (reason: string): void => {
    if (pendingToHelper !== null) {
      options.logger.info(`[rd-media-relay] pending clipboard text discarded: ${reason}`);
      pendingToHelper = null;
    }
    if (pendingToController !== null) {
      options.logger.info(`[rd-media-relay] pending clipboard text (upstream) discarded: ${reason}`);
      pendingToController = null;
    }
  };

  return {
    handleHelperText(text) {
      // 被控端**本端**剪贴板变了：observeLocal 语义（登记后同值不再上行）。
      if (sync.observeLocal(text) !== "push") {
        return;
      }
      // 登记发生在发送之前是刻意的：发不出去的由 pendingToController 兜住，不是丢在地上
      //（媒体 WS 断连重连期间 helper 仍在轮询，这是产品里可达的窗口，见 pendingToController）。
      pushToController(text);
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
    flushPendingToHelper() {
      if (pendingToHelper !== null) {
        writeToHelper(pendingToHelper);
      }
    },
    flushPendingToController() {
      if (pendingToController !== null) {
        pushToController(pendingToController);
      }
    },
    clearPending() {
      discardPending("session closed");
    },
    handleModeMessage(message) {
      const parsed = remoteDesktopClipboardModeSchema.safeParse(message);
      if (!parsed.success) {
        return false;
      }
      sync.setEnabled(parsed.data.enabled);
      if (!parsed.data.enabled) {
        // 关掉同步后不该再往被控剪贴板写、也不该再往主控剪贴板写（含未落地/未上行的留存值）。
        // 与「关闭不重置 lastSynced」不冲突：那条约定管的是重开后不补发关闭期间的旧值，
        // 这里做的正是同一件事。
        discardPending("clipboard disabled");
      }
      return true;
    },
    isEnabled() {
      return sync.isEnabled();
    },
  };
}
