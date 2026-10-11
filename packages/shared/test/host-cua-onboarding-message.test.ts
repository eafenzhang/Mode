import assert from "node:assert/strict";
import test from "node:test";
import { hostIncomingMessageSchema, hostResponseMessageSchema } from "../src/validation.js";

// host→main 打开对端 onboarding 的新消息：discriminated union 必须显式登记，
// 否则 main 的 safeParse 静默丢弃（与既有 LocalMediaPreviewPathAuthorizeRequest 同规）。
test("host 请求消息解析保留 requestId 与权限参数", () => {
  const parsed = hostResponseMessageSchema.safeParse({
    type: "host-open-cua-permission-onboarding-request",
    requestId: "req-1",
    initialPermission: "screenRecording",
    requiredPermissions: ["screenRecording", "accessibility"],
  });
  assert.equal(parsed.success, true);
});

test("未知 type 拒绝（fail closed，回归既有语义）", () => {
  const parsed = hostResponseMessageSchema.safeParse({ type: "host-not-a-real-type" });
  assert.equal(parsed.success, false);
});

// main→host 结果回包同规：host 侧入站也走 hostIncomingMessageSchema
// （hostMessagePortGuard），漏登记会让回包在校验层被丢弃、pending 只能等超时收敛。
test("main 结果回包解析保留 requestId 与 success/error", () => {
  const parsed = hostIncomingMessageSchema.safeParse({
    type: "open-cua-permission-onboarding-result",
    requestId: "req-1",
    success: false,
    error: "unavailable",
  });
  assert.equal(parsed.success, true);
  if (parsed.success) {
    assert.equal(parsed.data.requestId, "req-1");
  }
});
