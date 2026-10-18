import assert from "node:assert/strict";
import test from "node:test";

import { modelDisplayParts } from "../src/app-model-ref.js";

test("模型展示：无 provider 时用占位符", () => {
  assert.deepEqual(modelDisplayParts("gpt-5"), { model: "gpt-5", provider: "-" });
});

test("模型展示：按首个斜杠拆分 provider 与模型", () => {
  assert.deepEqual(modelDisplayParts("openai/gpt-5"), { model: "gpt-5", provider: "openai" });
  assert.deepEqual(modelDisplayParts("a/b/c"), { model: "b/c", provider: "a" });
});

test("模型展示：空选择返回全占位符", () => {
  assert.deepEqual(modelDisplayParts(""), { model: "-", provider: "-" });
});
