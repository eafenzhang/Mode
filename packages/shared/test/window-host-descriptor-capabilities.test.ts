import assert from "node:assert/strict";
import test from "node:test";
import { windowHostRemoteWorkspaceDescriptorSchema } from "../src/validation.js";

// 能力位经 host→main 的 descriptor 消息下发；strict schema 下新字段必须显式建模，
// 且旧消息（无 capabilities）必须继续解析（同进程版本一致，但保持 schema 前向可读）。
// 依据 docs/specs/lan-remote-computer-use.md「接口变更 1-2」「旧对端降级」。
const baseDescriptor = {
  remoteSessionId: "rs-1",
  target: { kind: "lan" as const, host: "192.168.1.10", port: 3030 },
  workspacePath: "/tmp/ws",
  workspaceIdentity: "remote:lan:192.168.1.10:3030:/tmp/ws",
  generation: 1,
};

test("descriptor 无 capabilities 仍解析通过", () => {
  const parsed = windowHostRemoteWorkspaceDescriptorSchema.safeParse(baseDescriptor);
  assert.equal(parsed.success, true);
});

test("descriptor 携带 computerUse 能力位解析保留", () => {
  const parsed = windowHostRemoteWorkspaceDescriptorSchema.safeParse({
    ...baseDescriptor,
    capabilities: { computerUse: true },
  });
  assert.equal(parsed.success, true);
  assert.equal(parsed.success && parsed.data.capabilities?.computerUse, true);
});

test("capabilities.computerUse 非布尔解析失败（fail closed）", () => {
  const parsed = windowHostRemoteWorkspaceDescriptorSchema.safeParse({
    ...baseDescriptor,
    capabilities: { computerUse: 1 },
  });
  assert.equal(parsed.success, false);
});
