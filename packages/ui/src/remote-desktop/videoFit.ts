/**
 * 视频面几何与指针映射的 DOM 侧薄封装（纯函数在 `@mode/shared/remote-desktop`，
 * 单测见 packages/shared/test/remote-desktop.test.ts；这里只做「DOM → 纯函数入参」的取值）。
 *
 * 规则（docs/specs/lan-remote-desktop.md 产品规则 6）：画面按视频实际尺寸 letterbox 适配，
 * 鼠标坐标以画面归一化（x/y ∈ [0,1]）；letterbox 区域内的点 clamp 到边缘（纯函数负责）。
 */
import { computeNormalizedPointFromVideoEvent } from "@mode/shared/remote-desktop";

export interface RemoteDesktopNormalizedPoint {
  x: number;
  y: number;
  inside: boolean;
}

function readVideoIntrinsicSize(video: HTMLVideoElement): { width: number; height: number } | null {
  if (video.videoWidth <= 0 || video.videoHeight <= 0) {
    return null;
  }
  return { width: video.videoWidth, height: video.videoHeight };
}

/** 指针在视频元素内的位置 → 画面归一化坐标；视频元数据未就绪时返回 null（不发送）。 */
export function normalizedPointFromVideoPointer(
  video: HTMLVideoElement,
  client: { x: number; y: number },
): RemoteDesktopNormalizedPoint | null {
  const intrinsic = readVideoIntrinsicSize(video);
  if (!intrinsic) {
    return null;
  }
  const rect = video.getBoundingClientRect();
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
