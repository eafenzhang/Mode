// CLI 层：agent-server 命令解析优先级与用法错误（spec §3）。
import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  EvalConfigError,
  resolveAgentServerLaunch,
  runEvalCommand,
  type EvalCommandContext,
} from "../src/cli.js";

function captureCtx(): { ctx: EvalCommandContext; stdout: string[]; stderr: string[] } {
  const stdout: string[] = [];
  const stderr: string[] = [];
  return {
    ctx: {
      stdout: { write: (chunk: string) => (stdout.push(chunk), true) } as unknown as NodeJS.WritableStream,
      stderr: { write: (chunk: string) => (stderr.push(chunk), true) } as unknown as NodeJS.WritableStream,
    },
    stdout,
    stderr,
  };
}

const BIN = process.platform === "win32" ? "agent-server.exe" : "agent-server";

test("解析优先级：flag → MODE_EVAL → MODE_AGENT_SERVER(+args JSON) → 向上搜索", async () => {
  const root = await mkdtemp(join(tmpdir(), "mode-eval-cli-"));
  try {
    const nested = join(root, "a", "b");
    await mkdir(join(root, "target", "debug"), { recursive: true });
    await writeFile(join(root, "target", "debug", BIN), "", "utf8");

    const byFlag = resolveAgentServerLaunch({ flag: "/opt/agent-server", env: {}, cwd: nested });
    assert.deepEqual(byFlag, { command: "/opt/agent-server", args: ["app-server", "--stdio"] });

    const byEvalEnv = resolveAgentServerLaunch({
      env: { MODE_EVAL_AGENT_SERVER_COMMAND: "/x/agent-server" },
      cwd: nested,
    });
    assert.equal(byEvalEnv.command, "/x/agent-server");

    const byHostEnv = resolveAgentServerLaunch({
      env: {
        MODE_AGENT_SERVER_COMMAND: "/y/agent-server",
        MODE_AGENT_SERVER_ARGS_JSON: '["app-server","--stdio","--extra"]',
      },
      cwd: nested,
    });
    assert.deepEqual(byHostEnv, { command: "/y/agent-server", args: ["app-server", "--stdio", "--extra"] });

    const bySearch = resolveAgentServerLaunch({ env: {}, cwd: nested });
    assert.equal(bySearch.command, join(root, "target", "debug", BIN));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("找不到 agent-server → EvalConfigError(MODE_EVAL_AGENT_SERVER_UNAVAILABLE)", () => {
  const root = mkdtempSyncRemove();
  assert.throws(
    () => resolveAgentServerLaunch({ env: {}, cwd: root }),
    (error: unknown) => error instanceof EvalConfigError && error.code === "MODE_EVAL_AGENT_SERVER_UNAVAILABLE",
  );
});

/** 深层临时目录：从它向上搜不到任何 target/。 */
function mkdtempSyncRemove(): string {
  // 用异步 mkdtemp 的同步替代：这里只构造一个一定不存在的深层路径。
  return join(tmpdir(), `mode-eval-missing-${Date.now()}`, "a", "b", "c");
}

test("用法错误：未知子命令 / 缺路径", async () => {
  const { ctx, stderr } = captureCtx();
  assert.equal(await runEvalCommand(ctx, {}, []), 1);
  assert.ok(stderr[0]?.includes("unknown subcommand"));
  assert.equal(await runEvalCommand(ctx, {}, ["run"]), 1);
  assert.ok(stderr[1]?.includes("missing <path>"));
});

test("--eval-timeout 非数字被拒绝", async () => {
  const { ctx, stderr } = captureCtx();
  assert.equal(await runEvalCommand(ctx, { evalTimeout: "abc" }, ["run", "x.json"]), 1);
  assert.ok(stderr[0]?.includes("--eval-timeout"));
});
