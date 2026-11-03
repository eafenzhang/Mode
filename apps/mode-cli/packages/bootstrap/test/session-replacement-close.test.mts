/**
 * `session/create` 同 id 重导时回收被顶替的 runtime。
 *
 * 远程工作区对话镜像用 `{sessionId: "mirror-<origin>", importedHistory}` 反复导入同一个会话；
 * 旧 record 被新 record `set` 顶替后会从 resident registry 消失，常驻池再也看不到它。
 * 若不在替换点关闭旧 runtime，它的 MCP stdio 子进程与连接就没有第二个回收点——实测每轮
 * 泄漏一个 `rea.mjs mcp`（约 0.24GB），50+ 个之后 Windows 提交内存耗尽、dwm 被杀（黑屏）。
 *
 * 见 docs/specs/session-write-group.md §0a 与
 * docs/specs/remote-workspace-conversation-mirror.md「2026-11-03 第六轮修复」。
 *
 * 运行：pnpm --filter @mode/bootstrap test
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { closeReplacedSessionRecord } from "../src/mode-protocol/session-residency.js";
import type { ModeProtocolSessionRecord } from "../src/mode-protocol/server-types.js";

type ReplacedRecord = Pick<ModeProtocolSessionRecord, "unsubscribe" | "app">;

function fakeRecord(params: {
  calls: string[];
  closeError?: Error;
  withUnsubscribe?: boolean;
}): ReplacedRecord {
  const app = {
    async close() {
      params.calls.push("close");
      if (params.closeError) {
        throw params.closeError;
      }
    },
  };
  const record: Record<string, unknown> = { app };
  if (params.withUnsubscribe !== false) {
    record.unsubscribe = () => params.calls.push("unsubscribe");
  }
  return record as unknown as ReplacedRecord;
}

test("替换 record 时先解订阅再关闭旧 runtime", async () => {
  const calls: string[] = [];
  await closeReplacedSessionRecord(fakeRecord({ calls }));
  assert.deepEqual(calls, ["unsubscribe", "close"]);
});

test("旧 runtime 关闭失败不回滚新 record：替换清理是 best-effort", async () => {
  const calls: string[] = [];
  await assert.doesNotReject(
    closeReplacedSessionRecord(fakeRecord({ calls, closeError: new Error("boom") })),
  );
  assert.deepEqual(calls, ["unsubscribe", "close"]);
});

test("没有 unsubscribe 的 record 仍然关闭旧 runtime", async () => {
  const calls: string[] = [];
  await closeReplacedSessionRecord(fakeRecord({ calls, withUnsubscribe: false }));
  assert.deepEqual(calls, ["close"]);
});
