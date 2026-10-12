/**
 * 媒体面几何与指针映射的 DOM 侧薄封装（纯函数在 `@mode/shared/remote-desktop`，
 * 单测见 packages/shared/test/remote-desktop.test.ts；这里只做「DOM → 纯函数入参」的取值）。
 *
 * 规则（docs/specs/lan-remote-desktop-native-media.md：画面按原生尺寸 letterbox 适配，
 * 鼠标坐标以画面归一化（x/y ∈ [0,1]）；letterbox 区域内的点 clamp 到边缘（纯函数负责）。
 */
import { computeNormalizedPointFromVideoEvent } from "@mode/shared/remote-desktop";

export interface RemoteDesktopNormalizedPoint {
  x: number;
  y: number;
  inside: boolean;
}

/**
 * 画面载体：WebRTC 期是 `<video>`，原生媒体面是解码直画的 `<canvas>`——两者的内在尺寸
 * 都是「画面像素」口径（video = 元数据尺寸；canvas = width/height 位图尺寸），letterbox
 * 归一化对二者同一套算法。
 */
export type RemoteDesktopMediaSurface = HTMLVideoElement | HTMLCanvasElement;

function readSurfaceIntrinsicSize(
  surface: RemoteDesktopMediaSurface,
): { width: number; height: number } | null {
  // 无 DOM 全局的运行时（Node 单测）没有 HTMLVideoElement：typeof 守卫使判断退化为
  // canvas 分支；浏览器里该全局必然存在，instanceof 语义不变。
  if (typeof HTMLVideoElement !== "undefined" && surface instanceof HTMLVideoElement) {
    if (surface.videoWidth <= 0 || surface.videoHeight <= 0) {
      return null;
    }
    return { width: surface.videoWidth, height: surface.videoHeight };
  }
  if (surface.width <= 0 || surface.height <= 0) {
    return null;
  }
  return { width: surface.width, height: surface.height };
}

/** 指针在画面元素内的位置 → 画面归一化坐标；画面内在尺寸未就绪（无帧/无元数据）时返回 null（不发送）。 */
export function normalizedPointFromVideoPointer(
  surface: RemoteDesktopMediaSurface,
  client: { x: number; y: number },
): RemoteDesktopNormalizedPoint | null {
  const intrinsic = readSurfaceIntrinsicSize(surface);
  if (!intrinsic) {
    return null;
  }
  const rect = surface.getBoundingClientRect();
  // 画面元素之外（顶部操作栏/底部状态条）必须返回 null：纯函数对界外点会 clamp 回
  // 画面边缘，InputCapture 会据此对工具栏的按下 preventDefault + setPointerCapture，
  // 把 click 劫持到容器（按钮点不动），移动/滚轮还会误发给对端（spec UI 规则·会话面：
  // 操作栏与状态条区域的指针/按键只走本地，不转发）。letterbox 黑边在元素内，维持 clamp。
  if (
    client.x < rect.left ||
    client.x > rect.left + rect.width ||
    client.y < rect.top ||
    client.y > rect.top + rect.height
  ) {
    return null;
  }
  return computeNormalizedPointFromVideoEvent(
    { left: rect.left, top: rect.top, width: rect.width, height: rect.height },
    intrinsic,
    client,
  );
}

export type RemoteDesktopWheelDirection = "up" | "down" | "left" | "right";

/** 滚轮方向：主轴优先（横向滚动取 |deltaX| 占优时）；两轴都为 0 时返回 null（不下发）。 */
export function wheelDirectionFromDelta(
  deltaX: number,
  deltaY: number,
): RemoteDesktopWheelDirection | null {
  const absX = Math.abs(deltaX);
  const absY = Math.abs(deltaY);
  if (absX === 0 && absY === 0) {
    return null;
  }
  if (absX > absY) {
    return deltaX < 0 ? "left" : "right";
  }
  return deltaY < 0 ? "up" : "down";
}

/** 滚轮刻度原始量（被控端按像素/行两种口径归一，见 inputConversion.normalizeWheelAmount）。 */
export function wheelDeltaMagnitude(deltaX: number, deltaY: number): number {
  return Math.max(Math.abs(deltaX), Math.abs(deltaY));
}
