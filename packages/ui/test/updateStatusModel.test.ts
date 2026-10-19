/**
 * 更新弹窗进度模型：progress 是字符串（"39" / "39%"），必须落到进度条数值。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { deriveUpdateStatusViewModel } from "../src/updateStatusModel.js";

function view(progress: string, bytes?: { transferredBytes?: number; totalBytes?: number }) {
  return deriveUpdateStatusViewModel({
    legacyReadyVersion: null,
    updateState: {
      kind: "download-progress",
      enabled: true,
      progress,
      version: "0.0.60",
      ...bytes,
    },
  });
}

test("download-progress：纯数字与带百分号的 progress 都能驱动进度条", () => {
  assert.equal(view("39").progressValue, 39);
  assert.equal(view("39%").progressValue, 39);
  assert.equal(view("100%").progressValue, 100);
  assert.equal(view("0").progressValue, 0);
});

test("download-progress：progress 无效时回退到字节比例", () => {
  assert.equal(view("", { transferredBytes: 20, totalBytes: 50 }).progressValue, 40);
  assert.equal(view("NaN", { transferredBytes: 1, totalBytes: 4 }).progressValue, 25);
});
