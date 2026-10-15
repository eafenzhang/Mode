import assert from "node:assert/strict";
import test from "node:test";
import { serverRemoteInfoSchema } from "../src/server-remote.js";

// server-info capabilities 的 computerUse 位：旧对端缺键必须解析通过（握手降级为不支持），
// 新对端必须是布尔；非布尔 fail closed。依据 docs/specs/lan-remote-computer-use.md「旧对端降级」。
const baseInfo = {
  serverId: "srv-1",
  version: "1.0.0",
  protocolVersion: 1 as const,
  authRequired: false,
  workspaces: [],
  capabilities: { desktopContinuous: true as const, websocketRpc: true as const },
};

test("旧对端 capabilities 无 computerUse 键仍解析通过", () => {
  const parsed = serverRemoteInfoSchema.safeParse(baseInfo);
  assert.equal(parsed.success, true);
  assert.equal(parsed.success && parsed.data.capabilities.computerUse, undefined);
});

test("新对端 computerUse 布尔位保留", () => {
  const parsed = serverRemoteInfoSchema.safeParse({
    ...baseInfo,
    capabilities: { ...baseInfo.capabilities, computerUse: true },
  });
  assert.equal(parsed.success, true);
  assert.equal(parsed.success && parsed.data.capabilities.computerUse, true);
});

test("computerUse 非布尔解析失败（fail closed）", () => {
  const parsed = serverRemoteInfoSchema.safeParse({
    ...baseInfo,
    capabilities: { ...baseInfo.capabilities, computerUse: "yes" },
  });
  assert.equal(parsed.success, false);
});

// 原生媒体面版本门 remoteDesktopNative（docs/specs/lan-remote-desktop-native-media.md 版本门 1）：
// 与 computerUse 同口径——旧对端缺键解析通过（握手按不支持收口），非布尔 fail closed。
test("旧对端 capabilities 无 remoteDesktopNative 键仍解析通过（缺键=旧对端）", () => {
  const parsed = serverRemoteInfoSchema.safeParse(baseInfo);
  assert.equal(parsed.success, true);
  assert.equal(parsed.success && parsed.data.capabilities.remoteDesktopNative, undefined);
});

test("新对端 remoteDesktopNative 布尔位保留", () => {
  const parsed = serverRemoteInfoSchema.safeParse({
    ...baseInfo,
    capabilities: { ...baseInfo.capabilities, remoteDesktopNative: true },
  });
  assert.equal(parsed.success, true);
  assert.equal(parsed.success && parsed.data.capabilities.remoteDesktopNative, true);
});

test("remoteDesktopNative 非布尔解析失败（fail closed）", () => {
  const parsed = serverRemoteInfoSchema.safeParse({
    ...baseInfo,
    capabilities: { ...baseInfo.capabilities, remoteDesktopNative: "yes" },
  });
  assert.equal(parsed.success, false);
});
