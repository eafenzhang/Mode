/**
 * 被控端显示器枚举与物理矩形换算（独立模块；spec
 * docs/specs/lan-remote-desktop-native-media.md 删除与保留清单：P4 直替清理后保留——
 * MediaStart/MediaSwitch 的 rect 由本模块的物理矩形口径构成，monitors 信令枚举走本模块）。
 *
 * 物理像素口径：DIP bounds × scaleFactor（VIRTUALDESK 坐标域的构成单元）。
 * 已知限制（一期不改）：多显示器混合 DPI 下 DIP 布局与物理布局非线性，坐标换算按
 * 「单显示器 / 统一缩放」假设；混合 DPI 的精确映射留待两机清单校准。
 */
// electron 只在 listRemoteDesktopMonitors 的缺省实现里需要：本模块的纯函数
// （physicalRectOf / buildRemoteDesktopMonitors）要在无 Electron 运行时的 node --test
// 单测里可导入，故不做顶层静态 import（非 Electron 下静态具名导入会链接失败）。
import type { DesktopCapturerSource, Display } from "electron";
import type { RemoteDesktopMonitor } from "@mode/shared/remote-desktop";

/**
 * 显示器物理矩形（DIP bounds × scaleFactor，含多屏负原点）。
 * 与 host 侧换算模块（src/host/inputConversion.ts 的 PxRect）同形但各自声明：main 与 host 是
 * 两个独立 tsconfig rootDir，不能共享模块；字段口径两端对齐（本模块 physicalRectOf/unionRectOf
 * → 本目录 toMediaRect → 契约 F 的 rect/virtualRect → host toRemoteDesktopPxRect）。
 */
export interface RemoteDesktopPxRect {
  xPx: number;
  yPx: number;
  widthPx: number;
  heightPx: number;
}

export function displayIdOf(display: Display): string {
  return String(display.id);
}

/** 物理像素矩形（VIRTUALDESK 坐标域的构成单元）：DIP bounds × scaleFactor。 */
export function physicalRectOf(display: Display): RemoteDesktopPxRect {
  const scale = Number.isFinite(display.scaleFactor) && display.scaleFactor > 0 ? display.scaleFactor : 1;
  return {
    xPx: Math.round(display.bounds.x * scale),
    yPx: Math.round(display.bounds.y * scale),
    widthPx: Math.max(1, Math.round(display.bounds.width * scale)),
    heightPx: Math.max(1, Math.round(display.bounds.height * scale)),
  };
}

export function unionRectOf(displays: Display[]): RemoteDesktopPxRect {
  if (displays.length === 0) {
    return { xPx: 0, yPx: 0, widthPx: 1, heightPx: 1 };
  }
  const rects = displays.map(physicalRectOf);
  const left = Math.min(...rects.map((rect) => rect.xPx));
  const top = Math.min(...rects.map((rect) => rect.yPx));
  const right = Math.max(...rects.map((rect) => rect.xPx + rect.widthPx));
  const bottom = Math.max(...rects.map((rect) => rect.yPx + rect.heightPx));
  return { xPx: left, yPx: top, widthPx: Math.max(1, right - left), heightPx: Math.max(1, bottom - top) };
}

/** 显示器枚举：display_id 对不上时回退按顺序关联（部分平台 display_id 为空串）。 */
export function buildRemoteDesktopMonitors(
  sources: Array<Pick<DesktopCapturerSource, "id" | "name" | "display_id">>,
  displays: Display[],
  primaryDisplayId: string,
): RemoteDesktopMonitor[] {
  const usedDisplayIds = new Set<string>();
  const monitors: RemoteDesktopMonitor[] = [];
  sources.forEach((source, index) => {
    const byId = source.display_id
      ? displays.find((display) => displayIdOf(display) === source.display_id)
      : undefined;
    const display = byId ?? (!source.display_id ? displays[index] : undefined);
    if (!display) {
      return;
    }
    const displayId = displayIdOf(display);
    if (usedDisplayIds.has(displayId)) {
      return;
    }
    usedDisplayIds.add(displayId);
    const physical = physicalRectOf(display);
    monitors.push({
      displayId,
      label: source.name || `Screen ${index + 1}`,
      widthPx: physical.widthPx,
      heightPx: physical.heightPx,
      scaleFactor: display.scaleFactor > 0 ? display.scaleFactor : 1,
      primary: displayId === primaryDisplayId,
    });
  });
  return monitors;
}

export async function listRemoteDesktopMonitors(
  options: {
    captureSources?: () => Promise<DesktopCapturerSource[]>;
    displays?: () => Display[];
  } = {},
): Promise<RemoteDesktopMonitor[]> {
  const { desktopCapturer, screen } = await import("electron");
  const sources = await (
    options.captureSources ??
    (() => desktopCapturer.getSources({ types: ["screen"], thumbnailSize: { width: 0, height: 0 } }))
  )();
  const displays = (options.displays ?? (() => screen.getAllDisplays()))();
  return buildRemoteDesktopMonitors(sources, displays, displayIdOf(screen.getPrimaryDisplay()));
}
