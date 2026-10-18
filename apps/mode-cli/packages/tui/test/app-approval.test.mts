import assert from "node:assert/strict";
import test from "node:test";

import { approvalDecisionLabel, previewPermissionInput } from "../src/app-approval.js";

test("审批决策文案：仅识别显式 allow 值，其余回退 Deny", () => {
  assert.equal(approvalDecisionLabel("allow_once"), "Allow once");
  assert.equal(approvalDecisionLabel("allow_project"), "Always allow in this project");
  assert.equal(approvalDecisionLabel("deny"), "Deny");
  assert.equal(approvalDecisionLabel("allow_always"), "Deny");
  assert.equal(approvalDecisionLabel(undefined), "Deny");
});

test("权限输入预览：字符串原样、其余转为紧凑 JSON 文本", () => {
  assert.equal(previewPermissionInput("hello"), "hello");
  assert.equal(previewPermissionInput(42), "42");
  assert.equal(previewPermissionInput(null), "null");
  assert.equal(previewPermissionInput({ a: 1, b: "two" }), '{"a":1,"b":"two"}');
});

test("权限输入预览：长文本不截断", () => {
  const long = "x".repeat(400);
  assert.equal(previewPermissionInput(long).length, 400);
});
