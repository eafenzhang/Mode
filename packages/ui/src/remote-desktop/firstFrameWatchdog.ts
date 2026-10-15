/**
 * 首帧看门狗（spec: docs/specs/lan-remote-desktop-native-media.md「媒体面可观测性与首帧约束」）。
 *
 * WHY：媒体面对象建立即宣告 live（早于任何一帧），被控未出流时页面呈无提示纯黑，且双端日志零行
 * （2026-10-14 实机实证：画面已连接 / 0 fps / 0.0 Mbps / RTT 正常）。本模块把「开流后窗口内一帧
 * 未画」变成一次可日志、可上屏的事件，交给会话胶水只做三件事：open 起表、画帧停表、收口清表。
 */

/** 窗口：媒体 WS open 后超过此时长仍零帧即报（spec 定值，5s 与端到端主观可达性对齐）。 */
export const FIRST_FRAME_WATCHDOG_MS = 5_000;

/**
 * 判定（纯函数）：窗口内画过任意一帧就永不报——报的是「一帧都没有」，不是「帧率低」。
 */
export function shouldReportNoFirstFrame(elapsedMs: number, decodedFrames: number): boolean {
  return decodedFrames === 0 && elapsedMs >= FIRST_FRAME_WATCHDOG_MS;
}

export interface FirstFrameWatchdog {
  /** 新 socket 打开：清零本 socket 计数并起表（重连后的新 socket 重新计时）。 */
  arm(): void;
  /** 记一帧（解码即画后调用）：返回 true = 本 socket 首帧，调用方据此撤除「等待对端首帧」。 */
  noteFrame(): boolean;
  /** 关闭/收口：停表（此后不再触发回调；arm 会重新起表）。 */
  clear(): void;
}

export function createFirstFrameWatchdog(options: {
  /** 会话级累计收流字节（触发时报出，用于区分「没数据」与「有数据但解不出画面」）。 */
  bytesReceived: () => number;
  /** 会话已收口：定时器与收口竞争时的最后一道闸。 */
  isStopped: () => boolean;
  onNoFrames: (info: { waitedMs: number; bytesReceived: number }) => void;
}): FirstFrameWatchdog {
  let timer: ReturnType<typeof setTimeout> | null = null;
  /** 本 socket 已画帧数：看门狗判据（与调用方的会话级累计分开口径）。 */
  let framesForSocket = 0;
  let reportedForSocket = false;

  const clear = (): void => {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
  };

  return {
    arm(): void {
      clear();
      framesForSocket = 0;
      reportedForSocket = false;
      timer = setTimeout(() => {
        timer = null;
        if (options.isStopped() || reportedForSocket) {
          return;
        }
        if (!shouldReportNoFirstFrame(FIRST_FRAME_WATCHDOG_MS, framesForSocket)) {
          return;
        }
        // 每个 socket 至多报一次：重复上报只会刷屏，不带来新信息。
        reportedForSocket = true;
        options.onNoFrames({
          waitedMs: FIRST_FRAME_WATCHDOG_MS,
          bytesReceived: options.bytesReceived(),
        });
      }, FIRST_FRAME_WATCHDOG_MS);
    },
    noteFrame(): boolean {
      framesForSocket += 1;
      const isFirstForSocket = framesForSocket === 1;
      // 首帧即停表：本 socket 已自证有画面，窗口不再有意义。
      if (isFirstForSocket) {
        clear();
      }
      return isFirstForSocket;
    },
    clear,
  };
}
