/** 画面面指针映射的界外拒绝（spec: lan-remote-desktop UI 规则·会话面「操作栏只走本地」）。
 * 画面载体在原生媒体面下是 canvas（内在尺寸 = 解码帧尺寸），stub 按 canvas 形状构造。
 * 运行：pnpm exec tsx --tsconfig packages/ui/tsconfig.json --test packages/ui/test/remoteDesktopVideoFit.test.ts
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { normalizedPointFromVideoPointer } from "../src/remote-desktop/videoFit.js";

// 16:9 画面挂在 800×450 的元素矩形上（left=100, top=60）：内容恰好满幅。
function makeCanvas(width = 1920, height = 1080) {
  return {
    width,
    height,
    getBoundingClientRect: () => ({
      left: 100,
      top: 60,
      width: 800,
      height: 450,
      right: 900,
      bottom: 510,
    }),
  } as unknown as HTMLCanvasElement;
}

test("画面元素之外的点返回 null（操作栏/状态条不映射、不转发）", () => {
  const canvas = makeCanvas();
  // 上方操作栏 / 下方状态条 / 左右界外：全部拒绝——若返回 clamp 坐标，InputCapture 会对
  // 工具栏的按下 preventDefault + setPointerCapture，把 click 劫持到容器（按钮点不动）。
  assert.equal(normalizedPointFromVideoPointer(canvas, { x: 400, y: 30 }), null);
  assert.equal(normalizedPointFromVideoPointer(canvas, { x: 400, y: 520 }), null);
  assert.equal(normalizedPointFromVideoPointer(canvas, { x: 50, y: 200 }), null);
  assert.equal(normalizedPointFromVideoPointer(canvas, { x: 950, y: 200 }), null);
  // 矩形边界本身属于界外（与 rects 的闭区间判定一致）。
  assert.equal(normalizedPointFromVideoPointer(canvas, { x: 400, y: 59 }), null);
});

test("画面元素内的点照常映射（letterbox 黑边维持 clamp 到边缘）", () => {
  const canvas = makeCanvas();
  const center = normalizedPointFromVideoPointer(canvas, { x: 500, y: 285 });
  assert.ok(center);
  assert.equal(center.inside, true);
  assert.equal(center.x, 0.5);
  assert.equal(center.y, 0.5);
  const corner = normalizedPointFromVideoPointer(canvas, { x: 100, y: 60 });
  assert.ok(corner);
  assert.equal(corner.x, 0);
  assert.equal(corner.y, 0);
  assert.equal(corner.inside, true);
});

test("4:3 素材在 16:9 元素内：左右黑边的点 clamp 到画面边缘且仍视为映射", () => {
  // 素材 1200×900 挂在 800×450：scale=min(800/1200,450/900)=0.5 → 内容 600×450 居中，
  // 左黑边范围 x∈[100,200)。
  const canvas = makeCanvas(1200, 900);
  const inLeftBar = normalizedPointFromVideoPointer(canvas, { x: 150, y: 285 });
  assert.ok(inLeftBar);
  assert.equal(inLeftBar.x, 0);
  assert.equal(inLeftBar.inside, false);
});

test("画面内在尺寸未就绪（无帧）不发送", () => {
  const canvas = makeCanvas(0, 0);
  assert.equal(normalizedPointFromVideoPointer(canvas, { x: 500, y: 285 }), null);
});
