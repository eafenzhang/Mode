/**
 * 被控 host 侧输入回转换算与批量（纯逻辑，单测见 tests/fixtures/remote-desktop-input.ts）。
 *
 * 链路（Task 14 fix round 1：换算单跳落在 host，main 不再参与注入）：
 *   主控 renderer（media WS 上行 JSON {t:...}，显示器内归一化 0..1）
 *     → host remoteDesktopServer.onInput（契约 C schema 校验）
 *     → 本模块换算（monitor + virtualRect 由 MediaStart/MediaSwitch 携带）
 *     → 既有注入端口（helper raw_input，VIRTUALDESK 绝对坐标）
 *
 * - 坐标：主控按「所选显示器画面」归一化（0..1）；这里用 shared `mapMonitorNormalizedToVirtualAbsolute`
 *   换算为 VIRTUALDESK 绝对坐标（0..65535，含多屏负原点）。
 * - 批量：pointer 事件 0ms 聚合（仅同 tick 合并，主控已即时发送，见
 *   docs/specs/lan-remote-desktop-native-media.md 输入面）；control 事件即时触发一次 flush
 *   （先 pointer 后 control，保持同帧内先后）。单帧 ≤512 条（与 Rust MAX_RAW_INPUT_EVENTS 同口径）。
 */
import {
  mapMonitorNormalizedToVirtualAbsolute,
  type RemoteDesktopControlEvent,
  type RemoteDesktopPointerEvent,
} from "@mode/shared/remote-desktop";
import {
  REMOTE_DESKTOP_INJECT_MAX_EVENTS,
  type RemoteDesktopRawInputEvent,
} from "./remoteDesktopMessages.js";
export { REMOTE_DESKTOP_INJECT_MAX_EVENTS };

/** 显示器物理矩形（被控端 screen 模块口径，含负原点）。 */
export interface RemoteDesktopPxRect {
  xPx: number;
  yPx: number;
  widthPx: number;
  heightPx: number;
}

/** host 契约的物理矩形（x/y/width/height，MediaStart/MediaSwitch 载荷）→ 换算用 PxRect。 */
export function toRemoteDesktopPxRect(rect: {
  x: number;
  y: number;
  width: number;
  height: number;
}): RemoteDesktopPxRect {
  return { xPx: rect.x, yPx: rect.y, widthPx: rect.width, heightPx: rect.height };
}

/**
 * pointer 聚合窗口：0ms = 只在同一个事件循环 tick 内合并（setTimeout(0) 调度），
 * 不再人为排队 16ms——主控已即时发送，这段等待是可削的指针端到端延迟
 * （lan-remote-desktop-native-media.md 输入面；爆量仍由 512 条/帧上限立即 flush 兜底）。
 */
export const REMOTE_DESKTOP_POINTER_FRAME_MS = 0;

function toAbsolute(
  point: { x: number; y: number },
  monitor: RemoteDesktopPxRect,
  virtual: RemoteDesktopPxRect,
): { x: number; y: number } {
  return mapMonitorNormalizedToVirtualAbsolute(point, monitor, virtual);
}

/** pointer 通道（pm）：单条 move，坐标换算到 VIRTUALDESK 口径。 */
export function convertPointerEvent(
  event: RemoteDesktopPointerEvent,
  monitor: RemoteDesktopPxRect,
  virtual: RemoteDesktopPxRect,
): RemoteDesktopRawInputEvent[] {
  const point = toAbsolute(event, monitor, virtual);
  return [{ k: "move", x: point.x, y: point.y }];
}

/**
 * 滚轮 delta → 刻度数（Rust `amount` 单位 = WHEEL_DELTA 刻度，round(amount*120) 后为 0 则不下发）。
 * Chromium 的 deltaY 一行/一格约 100px（deltaMode=0）或 3 行（deltaMode=1）；事件不带 deltaMode，
 * 因此 ≥20 视作像素口径除以 100，否则视作行口径除以 3，并 clamp 到 [1/3, 20]（至少下发 1 刻度）。
 */
export function normalizeWheelAmount(delta: number): number {
  const magnitude = Math.abs(delta);
  if (!Number.isFinite(magnitude) || magnitude === 0) {
    return 0;
  }
  const notches = magnitude >= 20 ? magnitude / 100 : magnitude / 3;
  return Math.min(20, Math.max(1 / 3, notches));
}

/** control 通道：按钮 / 滚轮 / 按键 / 文本（不丢失，即时下发）。 */
export function convertControlEvent(
  event: RemoteDesktopControlEvent,
  monitor: RemoteDesktopPxRect,
  virtual: RemoteDesktopPxRect,
): RemoteDesktopRawInputEvent[] {
  switch (event.t) {
    case "md":
    case "mu": {
      const point = toAbsolute(event, monitor, virtual);
      return [{ k: "button", button: event.button, down: event.t === "md", x: point.x, y: point.y }];
    }
    case "mw": {
      const point = toAbsolute(event, monitor, virtual);
      const amount = normalizeWheelAmount(event.delta);
      if (amount === 0) {
        return [];
      }
      return [{ k: "wheel", direction: event.direction, amount, x: point.x, y: point.y }];
    }
    case "kd":
      return [{ k: "key", vk: event.vk, down: true }];
    case "ku":
      return [{ k: "key", vk: event.vk, down: false }];
    case "tx":
      return [{ k: "text", text: event.text }];
  }
}

/** 帧拆分：≤512 条/帧（超限拆多帧；单条永不拆分，文本事件的 4096 字符上限由 Rust 把关）。 */
export function splitRemoteDesktopInjectFrames(
  events: RemoteDesktopRawInputEvent[],
): RemoteDesktopRawInputEvent[][] {
  if (events.length <= REMOTE_DESKTOP_INJECT_MAX_EVENTS) {
    return events.length > 0 ? [events] : [];
  }
  const frames: RemoteDesktopRawInputEvent[][] = [];
  for (let index = 0; index < events.length; index += REMOTE_DESKTOP_INJECT_MAX_EVENTS) {
    frames.push(events.slice(index, index + REMOTE_DESKTOP_INJECT_MAX_EVENTS));
  }
  return frames;
}

export interface RemoteDesktopInjectBatcher {
  /** pointer 事件：进缓冲，等待 0ms 聚合窗口（同 tick 合并后 flush）。 */
  enqueuePointer(events: RemoteDesktopRawInputEvent[]): void;
  /** control 事件：即时 flush（缓冲的 pointer 先出，保持同帧先后）。 */
  enqueueControl(events: RemoteDesktopRawInputEvent[]): void;
  /** 会话收口/宿主不可用时清空缓冲（不补发，避免结束后的残留输入落到对端）。 */
  reset(): void;
  pendingCount(): number;
}

/**
 * 注入批量器：时间与调度全部注入（单测可控）。
 * `onFrame` 拿到已拆好的 ≤512 帧；超过上限时立即 flush 而非等待计时器（防内存堆积）。
 */
export function createRemoteDesktopInjectBatcher(options: {
  now: () => number;
  schedule: (callback: () => void, delayMs: number) => ReturnType<typeof setTimeout>;
  cancel: (timer: ReturnType<typeof setTimeout>) => void;
  onFrame: (events: RemoteDesktopRawInputEvent[]) => void;
}): RemoteDesktopInjectBatcher {
  let buffer: RemoteDesktopRawInputEvent[] = [];
  let timer: ReturnType<typeof setTimeout> | null = null;

  const cancelPending = (): void => {
    if (timer) {
      options.cancel(timer);
      timer = null;
    }
  };

  const flush = (): void => {
    cancelPending();
    const events = buffer;
    buffer = [];
    for (const frame of splitRemoteDesktopInjectFrames(events)) {
      options.onFrame(frame);
    }
  };

  const scheduleFlush = (): void => {
    if (timer) {
      return;
    }
    timer = options.schedule(() => {
      timer = null;
      flush();
    }, REMOTE_DESKTOP_POINTER_FRAME_MS);
  };

  return {
    enqueuePointer(events) {
      if (events.length === 0) {
        return;
      }
      buffer.push(...events);
      if (buffer.length >= REMOTE_DESKTOP_INJECT_MAX_EVENTS) {
        flush();
        return;
      }
      scheduleFlush();
    },
    enqueueControl(events) {
      if (events.length > 0) {
        buffer.push(...events);
      }
      flush();
    },
    reset() {
      cancelPending();
      buffer = [];
    },
    pendingCount: () => buffer.length,
  };
}
