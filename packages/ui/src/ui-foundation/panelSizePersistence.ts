// P1.2 面板尺寸持久化契约（spec: docs/specs/m4-ui-foundation.md §3）。
// 组件不直接落盘：调用方实现 PanelSizePersistence（store/settings/IPC 均可），
// 这里只提供 clamp 后的初始值解析与内存实现（测试/演示用）。

import { clampPanelSize, type ResizeBounds } from "./resizeMath.js";

export type PanelId = "terminal" | "sidePane";

export interface PanelSizePersistence {
  load(panel: PanelId): number | null;
  save(panel: PanelId, size: number): void;
}

/** 已保存值优先，缺失用 fallback；两者都 clamp 到当前 bounds。 */
export function resolveInitialPanelSize(
  stored: number | null,
  fallback: number,
  bounds: ResizeBounds,
): number {
  return clampPanelSize(stored ?? fallback, bounds);
}

export function createMemoryPanelSizePersistence(
  initial: Partial<Record<PanelId, number>> = {},
): PanelSizePersistence {
  const values = new Map<PanelId, number>(Object.entries(initial) as Array<[PanelId, number]>);
  return {
    load: (panel) => values.get(panel) ?? null,
    save: (panel, size) => {
      values.set(panel, size);
    },
  };
}
