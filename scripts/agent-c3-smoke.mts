// C3 Task 5：真实端点 smoke（**opt-in，人工凭据**；不进 CI / verify 门禁）。
//
// 在 C2 smoke（`agent-c2-smoke.mts`：文本 turn + Read turn + 双工具 turn）之上补
// C3 的两个真实端点场景（凭据、隔离与销毁纪律与 C2 smoke 同一口径）：
// 1. **goal 场景**：`session/goal` set → goal-continuation turn → completion
//    verifier（真实端点的第二个非流式请求）→ target complete → `session/read`
//    的 target 投影断言（status/tokensUsed 正数）。
// 2. **retry 场景**：普通 prompt 正常完成后断言 `turn.completed` 恰好一次——
//    真实端点的 5xx 注入不可控，重试断言只验证「成功 turn 的 usage/终态」与
//    `turn_usage` 行（HTTP 5xx 路径的注入对拍由本地 mock 回放覆盖，
//    `replay-c3.mts` 的 retry 用例；真实端点 smoke 不伪造失败）。
//
// 用法（从仓库根目录；全部变量显式提供，缺一个即退出）：
//
//   MODE_SMOKE_PROVIDER=anthropic \        # anthropic（Messages）| responses（Responses）
//   MODE_SMOKE_BASE_URL=https://api.anthropic.com \
//   MODE_SMOKE_MODEL=<model-id> \
//   MODE_SMOKE_API_KEY=<api-key> \
//   [MODE_SMOKE_DATA_ROOT=<数据根>] \
//   pnpm exec tsx scripts/agent-c3-smoke.mts
//
// 纪律（与 `agent-c2-smoke.mts` 相同）：
// - 凭据只进 env 与本 smoke 自己生成的 provider 配置文件（隔离临时根内，随根销毁）；
//   不写日志、报告或 stdout 摘要。
// - 数据根默认 mkdtemp（跑完删除）；`MODE_SMOKE_DATA_ROOT` 显式提供的根不删除。
// - 绝不进 CI；不在任何 pnpm test / verify 流水线注册。
// - Rust 二进制不存在时直接失败（前置条件：`cargo build -p agent-server`）。
import { DatabaseSync } from "node:sqlite";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { assertDataRootIsolated, seedWorkspace } from "../packages/desktop/tests/agent-runtime-oracle/driver.mjs";
import { delay, fail, requireEnv, startSmokeServer, waitForTerminal, type SmokeSession } from "./agent-server-smoke-session.mjs";

const DATA_ROOT_PREFIX = "mode-agent-c3-smoke-";
const WORKSPACE_PREFIX = "mode-agent-c3-smoke-ws-";

type ApiFormat = "anthropic-messages" | "openai-responses";

function resolveProvider(): ApiFormat {
  const provider = requireEnv("MODE_SMOKE_PROVIDER");
  if (provider === "anthropic") return "anthropic-messages";
  if (provider === "responses") return "openai-responses";
  fail(`MODE_SMOKE_PROVIDER 只支持 anthropic | responses，实际：${provider}`);
}

/** 与 C2 smoke 同形状的 provider 配置（apiFormat 按 MODE_SMOKE_PROVIDER 切换）。 */
async function writeC3SmokeProviderConfig(
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
            providerName: "Agent C3 Smoke",
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

function terminalBaseline(session: SmokeSession): number {
  return session.frames.filter(
    (frame) => frame.type === "turn.completed" || frame.type === "turn.failed",
  ).length;
}

function usageOf(payload: Record<string, unknown>): Record<string, number> {
  const usage = (payload.usage ?? {}) as Record<string, unknown>;
  const number = (key: string): number =>
    typeof usage[key] === "number" ? (usage[key] as number) : -1;
  return {
    modelRequestCount: number("modelRequestCount"),
    inputTokens: number("inputTokens"),
    outputTokens: number("outputTokens"),
    totalTokens: number("totalTokens"),
  };
}

function assertUsage(turn: string, usage: Record<string, number>): void {
  if (usage.inputTokens <= 0 || usage.outputTokens <= 0 || usage.totalTokens <= 0) {
    fail(`${turn} usage 异常（应为正数）：${JSON.stringify(usage)}`);
  }
  if (usage.modelRequestCount < 1) {
    fail(`${turn} modelRequestCount 异常：${JSON.stringify(usage)}`);
  }
}

async function main(): Promise<void> {
  const apiFormat = resolveProvider();
  const baseUrl = requireEnv("MODE_SMOKE_BASE_URL");
  const apiKey = requireEnv("MODE_SMOKE_API_KEY");
  const model = requireEnv("MODE_SMOKE_MODEL");
  const explicitRoot = process.env.MODE_SMOKE_DATA_ROOT?.trim();

  const dataRoot = explicitRoot ? resolve(explicitRoot) : await mkdtemp(join(tmpdir(), DATA_ROOT_PREFIX));
  assertDataRootIsolated(dataRoot);
  const ownedRoot = !explicitRoot;
  const cwd = await mkdtemp(join(tmpdir(), WORKSPACE_PREFIX));
  await seedWorkspace(cwd, { "hello.txt": "hello from c3 smoke\n" });
  const dbPath = join(dataRoot, ".mode", "cli", "db", "db.sqlite");
  const configPath = join(dataRoot, "c3-smoke-personal-provider.json");
  await writeC3SmokeProviderConfig(configPath, apiFormat, baseUrl, apiKey);

  const goalObjective = "阅读 hello.txt 并在最终回复中给出文件的第一行内容，然后结束目标";
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
      // —— goal 场景：set → continuation → verifier → complete ——
      const goalBaseline = terminalBaseline(session);
      const goalSet = (await session.request("session/goal", {
        sessionId: session.sessionId,
        action: "set",
        objective: goalObjective,
        inputId: "c3_smoke_goal_set",
      })) as { response?: string; startedTurn?: boolean; snapshot?: { session?: { target?: { status?: string } } } } | undefined;
      if (goalSet?.startedTurn !== true) {
        fail(`session/goal set 未启动 turn：${JSON.stringify(goalSet)?.slice(0, 300)}`);
      }
      if (goalSet.snapshot?.session?.target?.status !== "active") {
        fail(`session/goal set 后目标不是 active：${JSON.stringify(goalSet.snapshot?.session?.target)}`);
      }
      summary.goalSet = { startedTurn: true, status: goalSet.snapshot?.session?.target?.status };
      // set → 控制 turn + goal-continuation turn + verifier：等 2 个终态
      //（控制 turn tokenCount 0 + continuation turn）。
      await waitForTerminal(session, goalBaseline + 1);
      await waitForTerminal(session, goalBaseline + 2);
      // verifier 通过后的 status_updated 可能落在第二个终态后的收尾窗里，轮询 read。
      let targetStatus = "";
      let tokensUsed = -1;
      for (let attempt = 0; attempt < 40; attempt += 1) {
        const read = (await session.request("session/read", { sessionId: session.sessionId })) as {
          session?: { target?: { status?: string; tokensUsed?: number } };
        };
        targetStatus = String(read.session?.target?.status ?? "");
        tokensUsed = typeof read.session?.target?.tokensUsed === "number" ? (read.session.target.tokensUsed as number) : -1;
        if (targetStatus === "complete") break;
        await delay(500);
      }
      if (targetStatus !== "complete") {
        fail(`goal 未在窗口内完成（target.status=${targetStatus}）`);
      }
      if (tokensUsed <= 0) {
        fail(`goal 完成 tokensUsed 异常：${tokensUsed}`);
      }
      summary.goalComplete = { status: targetStatus, tokensUsed };
      const showResult = (await session.request("session/goal", {
        sessionId: session.sessionId,
        action: "show",
        inputId: "c3_smoke_goal_show",
      })) as { response?: string };
      if (!String(showResult.response ?? "").startsWith("Goal complete\n")) {
        fail(`goal show 文案异常：${String(showResult.response).slice(0, 120)}`);
      }

      // —— retry/普通 turn 场景：成功终态恰好一次 + usage 断言 ——
      const promptBaseline = terminalBaseline(session);
      await session.request("session/send", {
        sessionId: session.sessionId,
        content: "请直接回复一句话：C3 smoke 普通回合成功。不要调用任何工具。",
      });
      const promptTerminal = await waitForTerminal(session, promptBaseline);
      const promptUsage = usageOf(promptTerminal.payload);
      assertUsage("普通 turn", promptUsage);
      const terminalCount = session.frames.filter(
        (frame) => frame.type === "turn.completed" || frame.type === "turn.failed",
      ).length;
      if (terminalCount !== promptBaseline + 1) {
        fail(`普通 turn 后终态数异常（可能出现重复终态）：${terminalCount} != ${promptBaseline + 1}`);
      }
      summary.promptTurn = { usage: promptUsage };

      // —— DB 行断言（C3 拥有的表）——
      await delay(500);
      const db = new DatabaseSync(dbPath, { readOnly: true });
      try {
        const targets = Number(db.prepare("SELECT COUNT(*) AS count FROM session_target").get().count);
        if (targets !== 1) fail(`session_target 行数异常（期望 1）：${targets}`);
        const targetRow = db.prepare("SELECT status, tokens_used FROM session_target").get() as {
          status: string;
          tokens_used: number;
        };
        if (targetRow.status !== "complete" || targetRow.tokens_used <= 0) {
          fail(`session_target 行内容异常：${JSON.stringify(targetRow)}`);
        }
        const turnUsage = Number(
          db.prepare("SELECT COUNT(*) AS count FROM turn_usage WHERE status = 'completed'").get().count,
        );
        if (turnUsage < 3) fail(`turn_usage(completed) 行数异常（期望 ≥3）：${turnUsage}`);
        summary.db = { sessionTarget: targetRow, turnUsageCompleted: turnUsage };
      } finally {
        db.close();
      }

      process.stdout.write(
        `C3 smoke 通过（${apiFormat}）\n${JSON.stringify(summary, null, 2)}\n` +
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
