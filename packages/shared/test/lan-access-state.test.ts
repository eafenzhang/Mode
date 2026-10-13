import assert from "node:assert/strict";
import test from "node:test";
import { lanAccessClientRecordSchema, lanAccessStateSchema } from "../src/lanAccess.js";
import { hostLanAccessStateResponseSchema } from "../src/validation.js";

// spec: docs/specs/lan-paired-devices.md「契约：LanAccessClientRecord 与状态读取」。
//
// 2026-10-13 回归背景：host 回帖的 lan client 记录携带 remoteDesktopAuthorized（逐设备
// 远程桌面授权，同意窗「记住此设备」/ 设置页开关写同一字段），而 shared 的 strict schema
// 漏声明该键——main 的 hostResponseMessageSchema 会整条拒收 lan-access-state-result，
// UI 表现为 `mode:get-lan-access-state` 8s 超时。本测试把「记录带该键时全链路 schema 可解析」
// 钉成契约：shared 记录 schema 与 host 回帖 schema 必须同步声明。

const baseClient = {
  id: "client-a",
  label: "Office-PC",
  createdAt: 1_700_000_000_000,
  lastUsedAt: null,
};

test("客户端记录：remoteDesktopAuthorized 可选三态（true/false/缺键）都能解析", () => {
  assert.equal(lanAccessClientRecordSchema.safeParse(baseClient).success, true, "缺键 = 未授权");
  assert.equal(
    lanAccessClientRecordSchema.safeParse({ ...baseClient, remoteDesktopAuthorized: true }).success,
    true,
  );
  assert.equal(
    lanAccessClientRecordSchema.safeParse({ ...baseClient, remoteDesktopAuthorized: false }).success,
    true,
  );
});

test("客户端记录：strict 仍拒绝未声明键（漏声明字段即拒收的根因保持不被误放行）", () => {
  assert.equal(
    lanAccessClientRecordSchema.safeParse({ ...baseClient, unknownKey: true }).success,
    false,
  );
});

test("host 回帖：带 remoteDesktopAuthorized 的 lan-access-state-result 必须整条可解析", () => {
  const response = {
    type: "lan-access-state-result",
    requestId: "r1",
    ok: true,
    state: {
      enabled: true,
      port: 45880,
      addresses: ["192.168.1.10"],
      pairCode: null,
      clients: [
        { ...baseClient, remoteDesktopAuthorized: true },
        { ...baseClient, id: "client-b", label: "Laptop" },
      ],
    },
  };
  const parsed = hostLanAccessStateResponseSchema.safeParse(response);
  assert.equal(parsed.success, true, "main 侧 host 回帖 schema 必须接受携带授权标记的记录");
  assert.equal(
    lanAccessStateSchema.safeParse(response.state).success,
    true,
    "LanAccessState 渲染层 schema 同口径",
  );
});
