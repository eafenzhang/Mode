/**
 * Host 传输死亡（post-boot failed）收口判定的单元用例（docs/specs/tauri-renderer-host.md §22）。
 *
 * 2026-10-25 验收缺陷的回归锁：Host 进程死亡后壳与桥都会发布 `phase:"failed"`，
 * 渲染层必须且只必须在「启动准入已完成 + 同代 startupId」时收口——漏判会回到
 * 全部 RPC 永久 pending 的静默挂起，误判会把 boot 期失败或异代状态错杀成本窗断连。
 */
import assert from "node:assert/strict";
import { test } from "node:test";

import { databaseStartupStateSchema, type DatabaseStartupState } from "@mode/shared";

import { isPostBootHostTransportDeath } from "../src/renderer/src/hostTransportDeath.js";

function buildState(overrides: Partial<DatabaseStartupState> = {}): DatabaseStartupState {
  return databaseStartupStateSchema.parse({
    schemaVersion: 1,
    startupId: "host-1-1000",
    attemptId: "host-1-1000-a1",
    sequence: 7,
    startedAt: 1_000,
    updatedAt: 2_000,
    phase: "failed",
    errorCode: "transport_closed",
    disk: [],
    ...overrides,
  });
}

test("post-boot 同代 failed：判定成立", () => {
  const decision = isPostBootHostTransportDeath(buildState(), {
    appInitialized: true,
    admittedStartupId: "host-1-1000",
    alreadyHandled: false,
  });
  assert.equal(decision, true);
});

test("boot 期（未完成准入）failed：不收口，走既有准入失败路径", () => {
  const decision = isPostBootHostTransportDeath(buildState(), {
    appInitialized: false,
    admittedStartupId: "host-1-1000",
    alreadyHandled: false,
  });
  assert.equal(decision, false);
});

test("异代 startupId：换代保护，不误杀本窗连接", () => {
  const decision = isPostBootHostTransportDeath(
    buildState({ startupId: "host-9-9999", attemptId: "host-9-9999-a1" }),
    {
      appInitialized: true,
      admittedStartupId: "host-1-1000",
      alreadyHandled: false,
    },
  );
  assert.equal(decision, false);
});

test("准入记录缺失（update-status 窗等）：不收口", () => {
  const decision = isPostBootHostTransportDeath(buildState(), {
    appInitialized: true,
    admittedStartupId: null,
    alreadyHandled: false,
  });
  assert.equal(decision, false);
});

test("非 failed（ready 等运行态快照）：不收口", () => {
  const decision = isPostBootHostTransportDeath(
    buildState({ phase: "ready", errorCode: undefined }),
    {
      appInitialized: true,
      admittedStartupId: "host-1-1000",
      alreadyHandled: false,
    },
  );
  assert.equal(decision, false);
});

test("已处理过（alreadyHandled）：幂等，不重复收口", () => {
  const decision = isPostBootHostTransportDeath(buildState(), {
    appInitialized: true,
    admittedStartupId: "host-1-1000",
    alreadyHandled: true,
  });
  assert.equal(decision, false);
});
