/**
 * 被控显示器枚举纯逻辑回归（spec: docs/specs/lan-remote-desktop-native-media.md
 * 删除与保留清单——captureWindow 在 P4 删除，显示器枚举抽出到独立模块保留）。
 * 运行：node --test packages/desktop/tests/remote-desktop-monitors.test.mts
 * 扩展名 .mts：语言政策新测试禁 .mjs（docs/specs/language-policy.md）；
 * 直接 import src 用 .ts 扩展名（node --test 原生 type stripping，与 remote-desktop-media-relay.test.mts 同口径）。
 */
import assert from "node:assert/strict";
import test from "node:test";
import type { DesktopCapturerSource, Display } from "electron";
import { buildRemoteDesktopMonitors, physicalRectOf } from "../src/main/remoteDesktop/monitors.ts";

/** Display 最小鸭子类型：physicalRectOf/buildRemoteDesktopMonitors 真正读到的三个字段。 */
function makeDisplay(over: {
  id?: number;
  bounds: { x: number; y: number; width: number; height: number };
  scaleFactor?: number;
}): Display {
  return { id: over.id ?? 1, bounds: over.bounds, scaleFactor: over.scaleFactor ?? 1 } as unknown as Display;
}

test("physicalRectOf：DIP bounds × scaleFactor 得物理像素矩形", () => {
  assert.deepEqual(
    physicalRectOf(makeDisplay({ bounds: { x: 0, y: 0, width: 1920, height: 1080 }, scaleFactor: 1.5 })),
    { xPx: 0, yPx: 0, widthPx: 2880, heightPx: 1620 },
    "1920×1080 @150% → 2880×1620",
  );
  // 多显示器负原点：DIP 坐标为负时物理坐标同步为负（VIRTUALDESK 换算的构成单元）。
  assert.deepEqual(
    physicalRectOf(makeDisplay({ id: 2, bounds: { x: -1920, y: 0, width: 1920, height: 1080 } })),
    { xPx: -1920, yPx: 0, widthPx: 1920, heightPx: 1080 },
  );
});

test("physicalRectOf：scaleFactor 非法（<=0 / NaN）回退 1，宽高至少 1", () => {
  assert.deepEqual(
    physicalRectOf(makeDisplay({ bounds: { x: 0, y: 0, width: 800, height: 600 }, scaleFactor: 0 })),
    { xPx: 0, yPx: 0, widthPx: 800, heightPx: 600 },
    "scaleFactor=0 回退 1",
  );
  assert.deepEqual(
    physicalRectOf(makeDisplay({ bounds: { x: 0, y: 0, width: 800, height: 600 }, scaleFactor: Number.NaN })),
    { xPx: 0, yPx: 0, widthPx: 800, heightPx: 600 },
    "scaleFactor=NaN 回退 1",
  );
  const degenerate = physicalRectOf(makeDisplay({ bounds: { x: 0, y: 0, width: 0, height: 0 } }));
  assert.equal(degenerate.widthPx, 1, "宽 0 收敛到 1（schema 要求正整数）");
  assert.equal(degenerate.heightPx, 1, "高 0 收敛到 1");
});

test("buildRemoteDesktopMonitors：display_id 关联、空 id 按序回退、去重与 primary 标记", () => {
  const displays = [
    makeDisplay({ id: 11, bounds: { x: 0, y: 0, width: 1920, height: 1080 }, scaleFactor: 1.5 }),
    makeDisplay({ id: 22, bounds: { x: 1920, y: 0, width: 1280, height: 720 } }),
  ];
  const source = (id: string, name: string, displayId: string): DesktopCapturerSource =>
    ({ id, name, display_id: displayId }) as unknown as DesktopCapturerSource;
  const monitors = buildRemoteDesktopMonitors(
    [
      source("screen:0:0", "", "11"),
      source("screen:1:0", "Screen 2", "22"),
      // 对不上任何 display 的源：跳过（不产生野 monitor）。
      source("screen:9:0", "Ghost", "99"),
    ],
    displays,
    "11",
  );
  assert.deepEqual(monitors, [
    { displayId: "11", label: "Screen 1", widthPx: 2880, heightPx: 1620, scaleFactor: 1.5, primary: true },
    { displayId: "22", label: "Screen 2", widthPx: 1280, heightPx: 720, scaleFactor: 1, primary: false },
  ]);

  // 部分平台 display_id 为空串：按顺序回退关联；同一 display 不重复登记。
  const fallback = buildRemoteDesktopMonitors(
    [
      source("screen:0:0", "First", ""),
      source("screen:1:0", "Second", ""),
      source("screen:2:0", "Dup", ""),
    ],
    displays,
    "22",
  );
  assert.deepEqual(
    fallback.map((monitor) => monitor.displayId),
    ["11", "22"],
    "空 display_id 按顺序关联且不重复登记同一 display",
  );
  assert.equal(fallback[1]?.primary, true, "primary 按 primaryDisplayId 判定");
});
