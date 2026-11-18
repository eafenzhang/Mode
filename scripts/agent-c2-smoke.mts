// C2 Task 8：真实端点 smoke（**opt-in，人工凭据**；不进 CI / verify 门禁）。
//
// 用显式提供的 Anthropic Messages 或 OpenAI Responses 真实端点跑 Rust `agent-server`：
// 文本 turn + Read 工具 turn +（Anthropic）**一次真实的双工具 turn**（T4 清单项：
// 同一 assistant 轮的多个 tool_use，其 tool_result 按「分组进同一条 user 消息」
// 发回——真实端点接受这条请求、turn 正常收口），并断言事件、usage 与 `db.sqlite` 行。
//
// 纪律（与 `agent-server-smoke.mts` 同一口径）：
// - **凭据只进 env 与进程环境**：API key 写进本 smoke 自己生成的 provider 配置文件
//   （落在隔离临时根内），不写进日志、报告或 stdout 摘要；退出时临时根连同配置
//   文件一起销毁。
// - **数据根隔离**：默认 `mkdtemp` 临时根（跑完删除）；T7 起 Rust 在空根上自迁移
//   （fresh-install），不需要预建基线库。显式给 `MODE_SMOKE_DATA_ROOT` 时使用
//   用户提供的根（同样由服务端自迁移；该根不会被本脚本删除）。
// - **真实端点绝不进 CI**：所有输入都来自 env，缺一个就退出；脚本不在任何
//   `pnpm test` / verify 流水线里注册。
// - Rust 二进制不存在时直接失败（前置条件：`cargo build -p agent-server`）。
//
// 用法（从仓库根目录；全部变量显式提供）：
//
//   MODE_SMOKE_PROVIDER=anthropic \        # anthropic（Messages）| responses（Responses）
//   MODE_SMOKE_BASE_URL=https://api.anthropic.com \   # anthropic：API 根（拼 /v1/messages）
//                                                    # responses：API 根（拼 /responses，如 https://api.openai.com/v1）
//   MODE_SMOKE_MODEL=<model-id> \
//   MODE_SMOKE_API_KEY=<api-key> \
//   [MODE_SMOKE_DATA_ROOT=<数据根>] \
//   pnpm exec tsx scripts/agent-c2-smoke.mts
//
// 断言面：
// - 每个 turn 的 `turn.completed` + usage（input/output/total 为正数、modelRequestCount ≥1）；
// - Read turn 观察 `tool.updated`（scheduled → result）与 `tool_usage` 落行；
// - Anthropic 双工具 turn：同一 turn 内 ≥2 次工具调用都执行完（tool_usage ≥2），
//   且第二个模型请求（分组 tool_result）被真实端点接受 → `turn.completed`；
// - DB：session/message/part/turn_usage/tool_usage 行数下限。
// 会话半边复用 `scripts/agent-server-smoke-session.mts`（stdio 驱动与终态解析是同一份）。
import { DatabaseSync } from "node:sqlite";
import { existsSync } from "node:fs";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { assertDataRootIsolated, seedWorkspace } from "../packages/desktop/tests/agent-runtime-oracle/driver.mjs";
import {
  delay,
  fail,
  requireEnv,
  startSmokeServer,
  usageOf,
  waitForTerminal,
  type SmokeSession,
} from "./agent-server-smoke-session.mjs";

const DATA_ROOT_PREFIX = "mode-agent-c2-smoke-";
const WORKSPACE_PREFIX = "mode-agent-c2-smoke-ws-";

const TEXT_PROMPT = "请直接回复一句话：C2 smoke 文本回合成功。不要调用任何工具。";
const READ_PROMPT = "用 Read 工具读一下 hello.txt，然后用一句话说明这个文件写了什么。";
const MULTI_PROMPT =
  "请并行调用两次 Read 工具，分别读取 hello.txt 和 world.txt，" +
  "然后用一句话同时说明两个文件的内容。";
const SEED = { "hello.txt": "hello from c2 smoke\n", "world.txt": "world from c2 smoke\n" };

type ApiFormat = "anthropic-messages" | "openai-responses";

function resolveProvider(): { apiFormat: ApiFormat; multiTool: boolean } {
  const provider = requireEnv("MODE_SMOKE_PROVIDER");
  if (provider === "anthropic") return { apiFormat: "anthropic-messages", multiTool: true };
  if (provider === "responses") return { apiFormat: "openai-responses", multiTool: false };
  fail(`MODE_SMOKE_PROVIDER 只支持 anthropic | responses，实际：${provider}`);
}

/** C2 smoke 自己的 personal provider 配置（apiFormat 按 MODE_SMOKE_PROVIDER 切换）。 */
async function writeC2SmokeProviderConfig(
  filePath: string,
  apiFormat: ApiFormat,
  baseUrl: string,
  apiKey: string,
): Promise<void> {
  const model = process.env.MODE_SMOKE_MODEL ?? "";
  const config = {
    schemaVersion: 1,
    config: {
      providerConfigRules: {
        providerRules: [
          {
            providerId: "smoke-provider",
            providerName: "Agent C2 Smoke",
            enabled: true,
            config: {
              group: "standard-personal",
              visibility: "visible",
              access: { type: "api-key", apiKey },
              personalModelIds: [model],
              api: { type: apiFormat, baseUrl },
            },
          },
        ],
      },
      modelConfigRules: {
        providerModelRules: [
          {
            providerId: "smoke-provider",
            modelId: model,
            config: {
              properties: {
                contextWindow: 200_000,
                supportsToolCall: true,
                supportsJsonSchemaOutput: false,
                supportsNativeWebSearch: false,
                supportsMidConversationSystem: true,
                inputFormat: { supportsText: true, supportsImage: false },
                outputFormat: { supportsText: true },
              },
              optionSpecs: {
                reasoningLevel: { values: ["disabled", "enabled"] },
                maxOutputTokens: { max: 8192 },
              },
            },
          },
        ],
        manualProviderModelRules: [],
      },
      providerOrder: ["smoke-provider"],
    },
  };
  await writeFile(filePath, `${JSON.stringify(config, null, 2)}\n`, "utf8");
}

function countTable(db: DatabaseSync, table: string, where: string): number {
  return Number(db.prepare(`SELECT COUNT(*) AS count FROM ${table} WHERE ${where}`).get().count);
}

function assertUsage(turn: string, usage: Record<string, number>): void {
  if (usage.inputTokens <= 0 || usage.outputTokens <= 0 || usage.totalTokens <= 0) {
    fail(`${turn} usage 异常（应为正数）：${JSON.stringify(usage)}`);
  }
  if (usage.modelRequestCount < 1) {
    fail(`${turn} modelRequestCount 异常：${JSON.stringify(usage)}`);
  }
}

function assertToolRan(session: SmokeSession, turn: string): void {
  const kinds = session.frames
    .filter((frame) => frame.type === "tool.updated")
    .map((frame) => String((frame.payload as Record<string, unknown>).kind));
  if (!kinds.includes("scheduled")) {
    fail(`${turn} 没有观察到 tool.updated(scheduled)：${JSON.stringify(session.frames.map((frame) => frame.type))}`);
  }
}

async function sendAndWait(
  session: SmokeSession,
  content: string,
  baseline: number,
): Promise<Record<string, unknown>> {
  await session.request("session/send", { sessionId: session.sessionId, content });
  const terminal = await waitForTerminal(session, baseline);
  return terminal.payload;
}

function terminalBaseline(session: SmokeSession): number {
  return session.frames.filter(
    (frame) => frame.type === "turn.completed" || frame.type === "turn.failed",
  ).length;
}

async function main(): Promise<void> {
  const { apiFormat, multiTool } = resolveProvider();
  const baseUrl = requireEnv("MODE_SMOKE_BASE_URL");
  const apiKey = requireEnv("MODE_SMOKE_API_KEY");
  const model = requireEnv("MODE_SMOKE_MODEL");
  const explicitRoot = process.env.MODE_SMOKE_DATA_ROOT?.trim();

  const dataRoot = explicitRoot ? resolve(explicitRoot) : await mkdtemp(join(tmpdir(), DATA_ROOT_PREFIX));
  assertDataRootIsolated(dataRoot);
  const ownedRoot = !explicitRoot;
  const cwd = await mkdtemp(join(tmpdir(), WORKSPACE_PREFIX));
  await seedWorkspace(cwd, SEED);
  const dbPath = join(dataRoot, ".mode", "cli", "db", "db.sqlite");
  if (explicitRoot && !existsSync(dbPath)) {
    // T7 起服务端在空根上自迁移；这里只挡「路径不是数据根」的拼错情形。
    process.stdout.write(`提示：${dbPath} 不存在，将由服务端 fresh-install 迁移创建。\n`);
  }
  const configPath = join(dataRoot, "c2-smoke-personal-provider.json");
  await writeC2SmokeProviderConfig(configPath, apiFormat, baseUrl, apiKey);

  const summary: Record<string, unknown> = {
    provider: process.env.MODE_SMOKE_PROVIDER,
    apiFormat,
    dataRoot,
    cwd,
    model,
  };
  try {
    const session = await startSmokeServer(dataRoot, cwd, configPath);
    try {
      // —— 文本 turn ——
      const textUsage = usageOf({ kind: "completed", payload: await sendAndWait(session, TEXT_PROMPT, 0) });
      assertUsage("文本 turn", textUsage);
      summary.textTurn = { usage: textUsage };

      // —— Read 工具 turn ——
      const readUsage = usageOf({
        kind: "completed",
        payload: await sendAndWait(session, READ_PROMPT, terminalBaseline(session)),
      });
      assertUsage("Read turn", readUsage);
      assertToolRan(session, "Read turn");
      summary.readTurn = { usage: readUsage };

      // —— 双工具 turn（T4 清单项；Anthropic 真实端点）——
      if (multiTool) {
        const multiPayload = await sendAndWait(session, MULTI_PROMPT, terminalBaseline(session));
        const multiUsage = usageOf({ kind: "completed", payload: multiPayload });
        assertUsage("双工具 turn", multiUsage);
        assertToolRan(session, "双工具 turn");
        // 两个工具结果都必须回到模型（分组 tool_result 被真实端点接受后才能引用）。
        const response = String(multiPayload.response ?? "");
        if (!response.toLowerCase().includes("hello") || !response.toLowerCase().includes("world")) {
          fail(
            `双工具 turn 的最终回复没有同时引用两个文件（分组 tool_result 可能没送达）：${response.slice(0, 200)}`,
          );
        }
        summary.multiToolTurn = {
          usage: multiUsage,
          responsePreview: response.slice(0, 120),
          note: "同一 assistant 轮的两个 tool_use 的 tool_result 分组进同一条 user 消息，真实端点接受并完成收口",
        };
      }

      // —— DB 行断言 ——
      await delay(500);
      const db = new DatabaseSync(dbPath, { readOnly: true });
      try {
        const sessions = countTable(db, "session", "1=1");
        const messages = countTable(db, "message", "1=1");
        const parts = countTable(db, "part", "1=1");
        const turnUsage = countTable(db, "turn_usage", "status = 'completed'");
        const toolUsage = countTable(db, "tool_usage", "1=1");
        const expectedTurns = multiTool ? 3 : 2;
        if (sessions < 1) fail(`session 行数异常：${sessions}`);
        if (messages < expectedTurns * 2) fail(`message 行数异常（期望 ≥${expectedTurns * 2}）：${messages}`);
        if (parts < expectedTurns * 2) fail(`part 行数异常（期望 ≥${expectedTurns * 2}）：${parts}`);
        if (turnUsage !== expectedTurns) fail(`turn_usage(completed) 行数异常（期望 ${expectedTurns}）：${turnUsage}`);
        if (toolUsage < (multiTool ? 3 : 1)) fail(`tool_usage 行数异常（期望 ≥${multiTool ? 3 : 1}）：${toolUsage}`);
        summary.db = { sessions, messages, parts, turnUsage, toolUsage };
      } finally {
        db.close();
      }

      process.stdout.write(
        `C2 smoke 通过（${apiFormat}）\n${JSON.stringify(summary, null, 2)}\n` +
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
      // 配置文件（含 API key）随临时根一起销毁。
    } else {
      process.stdout.write(`smoke 配置文件（含 API key）留在：${configPath}\n`);
    }
  }
}

await main();
