// C1 Task 10：真实 provider smoke（**opt-in**，不进 CI / verify 门禁）。
//
// 用显式提供的 OpenAI Chat Completions 兼容端点跑 Rust `agent-server`：
// 文本 turn + Read 工具 turn + 终态收口，并断言事件、usage 与 `db.sqlite` 行。
// 纪律：
// - **凭据只进 env 与进程环境**：API key 写进本 smoke 自己生成的 provider 配置文件
//   （落在隔离根内），不写进日志、报告或 stdout 摘要；退出时不清理配置文件所在的
//   临时根（配置文件随临时根一起销毁，见下）。
// - **数据根隔离**：默认 `mkdtemp` 临时根（跑完删除）；显式给 `MODE_SMOKE_DATA_ROOT`
//   时使用用户提供的**已迁移**数据根（C1 存储门 fail-closed，不会自建 schema）。
// - Rust 二进制不存在时直接失败（前置条件：`cargo build -p agent-server`）。
//
// 用法（全部变量显式提供，缺一个就退出）：
//
//   MODE_SMOKE_BASE_URL=https://.../v1 \
//   MODE_SMOKE_MODEL=<model-id> \
//   MODE_SMOKE_API_KEY=<api-key> \
//   [MODE_SMOKE_DATA_ROOT=<已迁移数据根的基目录>] \
//   pnpm exec tsx scripts/agent-server-smoke.mts
//
// `MODE_SMOKE_DATA_ROOT` 是**基目录**（会话库在 `<root>/.mode/cli/db/db.sqlite`），
// 与 `MODE_DATA_BASE_DIR` 的语义一致；不给则用隔离临时根（自动预建基线库）。
// 会话半边（provider 配置、stdio 驱动、终态/usage 解析）在
// `scripts/agent-server-smoke-session.mts`（拆文件守住 oxlint 的 max-lines）。
import { existsSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { assertDataRootIsolated, seedWorkspace } from "../packages/desktop/tests/agent-runtime-oracle/driver.mjs";
import { createBaselineDatabase } from "../packages/desktop/tests/agent-runtime-oracle/agent-server-env.mjs";
import {
  fail,
  requireEnv,
  startSmokeServer,
  usageOf,
  waitForTerminal,
  writeSmokeProviderConfig,
} from "./agent-server-smoke-session.mjs";

const DATA_ROOT_PREFIX = "mode-agent-server-smoke-";
const WORKSPACE_PREFIX = "mode-agent-server-smoke-ws-";

const TEXT_PROMPT = "请直接回复一句话：smoke 文本回合成功。不要调用任何工具。";
const READ_PROMPT =
  "用 Read 工具读一下 hello.txt，然后用一句话说明这个文件写了什么。";
const SEED = { "hello.txt": "hello from smoke\n" };

function countTable(db: DatabaseSync, table: string, where: string): number {
  return Number(
    db.prepare(`SELECT COUNT(*) AS count FROM ${table} WHERE ${where}`).get().count,
  );
}

async function main(): Promise<void> {
  const baseUrl = requireEnv("MODE_SMOKE_BASE_URL");
  const apiKey = requireEnv("MODE_SMOKE_API_KEY");
  const model = requireEnv("MODE_SMOKE_MODEL");
  const explicitRoot = process.env.MODE_SMOKE_DATA_ROOT?.trim();

  const dataRoot = explicitRoot
    ? resolve(explicitRoot)
    : await mkdtemp(join(tmpdir(), DATA_ROOT_PREFIX));
  assertDataRootIsolated(dataRoot);
  const ownedRoot = !explicitRoot;
  const cwd = await mkdtemp(join(tmpdir(), WORKSPACE_PREFIX));
  // 种子文件先落（Read turn 的目标）。
  await seedWorkspace(cwd, SEED);

  let dbPath: string;
  if (explicitRoot) {
    dbPath = join(dataRoot, ".mode", "cli", "db", "db.sqlite");
    if (!existsSync(dbPath)) {
      fail(`MODE_SMOKE_DATA_ROOT 下没有已迁移会话库：${dbPath}（C1 存储门 fail-closed，不自建 schema）`);
    }
  } else {
    dbPath = await createBaselineDatabase(dataRoot);
  }

  const configPath = join(dataRoot, "smoke-personal-provider.json");
  await writeSmokeProviderConfig(configPath, baseUrl, apiKey);

  const summary: Record<string, unknown> = {
    dataRoot,
    cwd,
    model,
    baseUrl,
  };
  try {
    const session = await startSmokeServer(dataRoot, cwd, configPath);
    try {
      // —— 文本 turn ——
      await session.request("session/send", { sessionId: session.sessionId, content: TEXT_PROMPT });
      const textTerminal = await waitForTerminal(session, 0);
      const textUsage = usageOf(textTerminal);
      if (textUsage.inputTokens <= 0 || textUsage.outputTokens <= 0) {
        fail(`文本 turn usage 异常（应为正数）：${JSON.stringify(textUsage)}`);
      }
      if (textUsage.modelRequestCount < 1) {
        fail(`文本 turn modelRequestCount 异常：${JSON.stringify(textUsage)}`);
      }
      const textResponse = String(textTerminal.payload.response ?? "");
      summary.textTurn = {
        responsePreview: textResponse.slice(0, 80),
        usage: textUsage,
      };

      // —— Read 工具 turn ——
      const baseline = session.frames.filter(
        (frame) => frame.type === "turn.completed" || frame.type === "turn.failed",
      ).length;
      await session.request("session/send", {
        sessionId: session.sessionId,
        content: READ_PROMPT,
      });
      const toolTerminal = await waitForTerminal(session, baseline);
      const toolUsage = usageOf(toolTerminal);
      const toolEvents = session.frames
        .filter((frame) => frame.type === "tool.updated")
        .map((frame) => String((frame.payload as Record<string, unknown>).kind));
      if (!toolEvents.includes("scheduled")) {
        fail(
          `Read turn 没有观察到 tool.updated(scheduled)：${JSON.stringify(session.frames.map((frame) => frame.type))}`,
        );
      }
      summary.readTurn = { toolEventKinds: toolEvents, usage: toolUsage };

      // —— DB 行断言（会话库：message / part / turn_usage）——
      const db = new DatabaseSync(dbPath, { readOnly: true });
      try {
        const sessions = countTable(db, "session", "1=1");
        const messages = countTable(db, "message", "1=1");
        const parts = countTable(db, "part", "1=1");
        const turnUsage = countTable(db, "turn_usage", "status = 'completed'");
        const toolUsage = countTable(db, "tool_usage", "1=1");
        if (sessions < 1) fail(`session 行数异常：${sessions}`);
        if (messages < 4) fail(`message 行数异常（期望 ≥4：2 user + 2 assistant）：${messages}`);
        if (parts < 6) fail(`part 行数异常（期望 ≥6）：${parts}`);
        if (turnUsage !== 2) fail(`turn_usage(completed) 行数异常（期望 2）：${turnUsage}`);
        if (toolUsage < 1) fail(`tool_usage 行数异常（期望 ≥1，Read 调用应落行）：${toolUsage}`);
        summary.db = { sessions, messages, parts, turnUsage, toolUsage };
      } finally {
        db.close();
      }

      process.stdout.write(
        `smoke 通过\n${JSON.stringify(summary, null, 2)}\n` +
          `事件总数：${session.frames.length}；事件类型：` +
          `${[...new Set(session.frames.map((frame) => frame.type))].join(",")}\n`,
      );
    } finally {
      await session.stop();
    }
  } finally {
    if (ownedRoot) {
      await rm(dataRoot, { recursive: true, force: true });
      await rm(cwd, { recursive: true, force: true });
      // 配置文件（含 API key）随临时根一起删除；显式数据根时配置文件留在用户目录里，
      // 由使用者自行处置——这里只提示路径。
    } else {
      process.stdout.write(`smoke 配置文件（含 API key）留在：${configPath}\n`);
    }
  }
}

await main();
