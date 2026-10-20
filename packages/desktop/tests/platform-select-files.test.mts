/**
 * 附件多选能力的暴露口径（spec: tauri-platform-commands §5）。
 *
 * Tauri 壳没有实现 `selectFiles` 命令。`IPlatformService` 上若保留一个“返回空数组”的
 * 空实现，`openAttachmentPicker` 会优先调用它并静默结束，不再回退到 `selectFile`
 * ——用户看到的就是 "+" 按钮无响应。
 */
import assert from "node:assert/strict";
import test from "node:test";

import { resolveSelectFilesCapability } from "../src/renderer/src/platformSelectFiles.js";

test("壳未实现 selectFiles 时不暴露该能力，picker 才能回退到 selectFile", () => {
  assert.deepEqual(resolveSelectFilesCapability(undefined), {});
  assert.equal("selectFiles" in resolveSelectFilesCapability(undefined), false);
});

test("壳实现 selectFiles 时按原函数透传", async () => {
  const capability = resolveSelectFilesCapability(async () => ["D:/shots/a.png"]);
  assert.equal(typeof capability.selectFiles, "function");
  assert.deepEqual(await capability.selectFiles?.(), ["D:/shots/a.png"]);
});
