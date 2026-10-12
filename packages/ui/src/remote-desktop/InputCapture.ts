/**
 * 主控输入采集：视频面上的 pointer / wheel / keyboard → T1 输入事件（DataChannel 双通道）。
 *
 * 通道纪律（docs/specs/lan-remote-desktop.md 事件顺序 ⑤ 与「质量与延迟参数」）：
 * - `pointer`（unordered + maxRetransmits:0）：移动采样可丢，**即时发送**（Chromium 按帧合并
 *   pointermove，这里不再叠加 rAF 节流——原先 ≤16.7ms 的排队是可削的指针延迟）；
 * - `control`（ordered + 可靠）：按钮 / 滚轮 / 按键 / 文本即时下发，不可丢。
 *
 * 键盘规则复用 shared `describeKeyAction`（spike 教训：文本与 VK 不能混发）：
 * - keydown：修饰键与组合键/不可打印键 → `kd`（VK）；纯可打印字符 → `tx`（Unicode 文本）；
 * - keyup：只对 VK 口径发 `ku`（文本键没有配对的按键抬起）。
 *
 * 释放纪律：失焦 / 指针捕获丢失时把仍按下的按钮与按键补发抬起，避免对端残留「按住」状态。
 * 本模块不碰 RTCPeerConnection 与 React：由 useRemoteDesktopSession 注入 sink。
 */
import type {
  RemoteDesktopControlEvent,
  RemoteDesktopMouseButton,
  RemoteDesktopPointerEvent,
} from "@mode/shared/remote-desktop";
import { describeKeyAction } from "@mode/shared/remote-desktop";
import {
  normalizedPointFromVideoPointer,
  wheelDeltaMagnitude,
  wheelDirectionFromDelta,
  type RemoteDesktopNormalizedPoint,
} from "./videoFit.js";

export interface RemoteDesktopInputSink {
  sendPointer(event: RemoteDesktopPointerEvent): void;
  sendControl(event: RemoteDesktopControlEvent): void;
}

export interface RemoteDesktopInputCaptureOptions {
  /** 焦点容器：键盘事件在这里采集（pointer 事件由视频元素冒泡上来）。 */
  container: HTMLElement;
  video: HTMLVideoElement;
  sink: RemoteDesktopInputSink;
}

export interface RemoteDesktopInputCapture {
  detach(): void;
}

const BUTTON_BY_INDEX: Record<number, RemoteDesktopMouseButton> = {
  0: "left",
  1: "middle",
  2: "right",
};

export function attachRemoteDesktopInput(
  options: RemoteDesktopInputCaptureOptions,
): RemoteDesktopInputCapture {
  const { container, video, sink } = options;

  const pressedButtons = new Set<RemoteDesktopMouseButton>();
  const pressedVks = new Set<number>();
  /** 最近一次有效画面坐标：失焦补发抬起时仍需落在原位置（否则会把对端光标拉到 0,0）。 */
  let lastPoint: RemoteDesktopNormalizedPoint | null = null;
  let detached = false;

  const resolvePoint = (event: { clientX: number; clientY: number }) => {
    const point = normalizedPointFromVideoPointer(video, { x: event.clientX, y: event.clientY });
    if (point) {
      lastPoint = point;
    }
    return point;
  };

  const releaseAll = (): void => {
    const point = lastPoint ?? { x: 0, y: 0, inside: false };
    for (const button of pressedButtons) {
      sink.sendControl({ t: "mu", button, x: point.x, y: point.y });
    }
    pressedButtons.clear();
    for (const vk of pressedVks) {
      sink.sendControl({ t: "ku", vk });
    }
    pressedVks.clear();
  };

  const onPointerDown = (event: PointerEvent): void => {
    const button = BUTTON_BY_INDEX[event.button];
    const point = resolvePoint(event);
    // 视频面上的点击都要落到对端：容器获得焦点以接收键盘（tabIndex=0）。
    container.focus({ preventScroll: true });
    if (!button || !point) {
      return;
    }
    event.preventDefault();
    pressedButtons.add(button);
    // 指针捕获：拖拽到画面外（窗口边缘）仍能收到 move/up。
    try {
      container.setPointerCapture(event.pointerId);
    } catch {
      // 捕获失败只影响窗口外拖拽，不影响本机画面内的操作。
    }
    sink.sendControl({ t: "md", button, x: point.x, y: point.y });
  };

  const onPointerMove = (event: PointerEvent): void => {
    if (detached) {
      return;
    }
    const point = resolvePoint(event);
    if (!point) {
      return;
    }
    // 即时发送：省掉 rAF 排队（≤16.7ms）；被控端同 tick 合并聚合（spec 质量与延迟参数）。
    sink.sendPointer({ t: "pm", x: point.x, y: point.y });
  };

  const onPointerUp = (event: PointerEvent): void => {
    const button = BUTTON_BY_INDEX[event.button];
    if (!button) {
      return;
    }
    event.preventDefault();
    const point = resolvePoint(event) ?? lastPoint ?? { x: 0, y: 0, inside: false };
    pressedButtons.delete(button);
    try {
      container.releasePointerCapture(event.pointerId);
    } catch {
      // 未捕获 / 已被浏览器释放时忽略。
    }
    sink.sendControl({ t: "mu", button, x: point.x, y: point.y });
  };

  const onPointerCancel = (): void => {
    releaseAll();
  };

  const onLostPointerCapture = (): void => {
    releaseAll();
  };

  const onWheel = (event: WheelEvent): void => {
    const direction = wheelDirectionFromDelta(event.deltaX, event.deltaY);
    const point = resolvePoint(event);
    if (!direction || !point) {
      return;
    }
    event.preventDefault();
    sink.sendControl({
      t: "mw",
      direction,
      delta: wheelDeltaMagnitude(event.deltaX, event.deltaY),
      x: point.x,
      y: point.y,
    });
  };

  const onKeyDown = (event: KeyboardEvent): void => {
    // 焦点在操作栏/下拉等本地控件上（target 非容器）：按键走本地默认行为、不下发对端——
    // 否则工具栏按钮的空格/回车激活与下拉方向键会被 preventDefault 吃掉（spec UI 规则·会话面）。
    if (event.target !== container) {
      return;
    }
    const action = describeKeyAction({
      key: event.key,
      keyCode: event.keyCode,
      ctrlKey: event.ctrlKey,
      altKey: event.altKey,
      metaKey: event.metaKey,
    });
    if (action.kind === "none") {
      return;
    }
    // 远程控制面接管全部按键：阻止本机默认行为（滚动、查找、缩进等）。
    event.preventDefault();
    if (action.kind === "text") {
      sink.sendControl({ t: "tx", text: action.text });
      return;
    }
    if (pressedVks.has(action.vk)) {
      // 自动重复：不重复发按下，避免对端 key-repeat 叠加。
      return;
    }
    pressedVks.add(action.vk);
    sink.sendControl({ t: "kd", vk: action.vk });
  };

  const onKeyUp = (event: KeyboardEvent): void => {
    // 与 keydown 同口径：本地控件持有的按键不下发（也就没有配对的 ku 需要发）。
    if (event.target !== container) {
      return;
    }
    const action = describeKeyAction({
      key: event.key,
      keyCode: event.keyCode,
      ctrlKey: event.ctrlKey,
      altKey: event.altKey,
      metaKey: event.metaKey,
    });
    // 文本输入没有配对的抬起（对端以 Unicode 文本注入），只有 VK 口径需要 ku。
    if (action.kind !== "vk") {
      return;
    }
    event.preventDefault();
    pressedVks.delete(action.vk);
    sink.sendControl({ t: "ku", vk: action.vk });
  };

  const onContextMenu = (event: MouseEvent): void => {
    // 右键菜单属于对端语义：本机不弹菜单，右键事件走 md/mu。
    event.preventDefault();
  };

  const onBlur = (): void => {
    releaseAll();
  };

  container.addEventListener("pointerdown", onPointerDown);
  container.addEventListener("pointermove", onPointerMove);
  container.addEventListener("pointerup", onPointerUp);
  container.addEventListener("pointercancel", onPointerCancel);
  container.addEventListener("lostpointercapture", onLostPointerCapture);
  container.addEventListener("wheel", onWheel, { passive: false });
  container.addEventListener("keydown", onKeyDown);
  container.addEventListener("keyup", onKeyUp);
  container.addEventListener("contextmenu", onContextMenu);
  container.addEventListener("blur", onBlur);

  return {
    detach() {
      if (detached) {
        return;
      }
      detached = true;
      container.removeEventListener("pointerdown", onPointerDown);
      container.removeEventListener("pointermove", onPointerMove);
      container.removeEventListener("pointerup", onPointerUp);
      container.removeEventListener("pointercancel", onPointerCancel);
      container.removeEventListener("lostpointercapture", onLostPointerCapture);
      container.removeEventListener("wheel", onWheel);
      container.removeEventListener("keydown", onKeyDown);
      container.removeEventListener("keyup", onKeyUp);
      container.removeEventListener("contextmenu", onContextMenu);
      container.removeEventListener("blur", onBlur);
      lastPoint = null;
      pressedButtons.clear();
      pressedVks.clear();
    },
  };
}
