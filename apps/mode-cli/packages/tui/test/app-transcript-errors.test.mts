import assert from "node:assert/strict";
import test from "node:test";

import { appendSystemErrorMessage } from "../src/app-transcript-errors.js";

test("系统错误消息：加 Error 前缀并以 system 角色追加", () => {
  assert.deepEqual(appendSystemErrorMessage([], "boom"), [
    { role: "system", content: "Error: boom" },
  ]);
});

test("系统错误消息：空文案回退 Unknown error", () => {
  assert.deepEqual(appendSystemErrorMessage([], ""), [
    { role: "system", content: "Error: Unknown error" },
  ]);
});

test("系统错误消息：保留原消息顺序且不修改入参数组", () => {
  const messages = [{ role: "user", content: "hi" }];
  const result = appendSystemErrorMessage(messages as never, "boom");
  assert.equal(result === (messages as never), false);
  assert.equal(messages.length, 1);
  assert.deepEqual(result[0], { role: "user", content: "hi" });
  assert.deepEqual(result[1], { role: "system", content: "Error: boom" });
});
