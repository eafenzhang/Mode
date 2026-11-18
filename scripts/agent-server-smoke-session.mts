// C1 Task 10：真实 provider smoke 的会话半边（provider 配置、stdio 驱动、终态/usage 解析）。
//
// 拆出本文件是为了守住 oxlint 的 `max-lines`（400 行，错误级），不是第二套纪律。
// 编排与断言在 `scripts/agent-server-smoke.mts`（main）：文本 turn + Read turn + DB 行断言。
import { spawn, type ChildProcess } from "node:child_process";
import { existsSync } from "node:fs";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = fileURLToPath(new URL("../", import.meta.url));

const PROVIDER_ID = "smoke-provider";
const REQUEST_TIMEOUT_MS = 60_000;
const TURN_TIMEOUT_MS = 120_000;
const STARTUP_TIMEOUT_MS = 60_000;

export type Frame = { type: string; payload: unknown };
export type Terminal = {
  kind: "completed" | "failed";
  payload: Record<string, unknown>;
};

export function fail(message: string): never {
  process.stderr.write(`smoke 失败：${message}\n`);
  process.exit(1);
}

export function requireEnv(name: string): string {
  const value = process.env[name]?.trim();
  if (!value) {
    process.stderr.write(
      `smoke 失败：缺少 ${name}。真实 provider smoke 需要：MODE_SMOKE_BASE_URL / MODE_SMOKE_MODEL / ` +
        "MODE_SMOKE_API_KEY（可选 MODE_SMOKE_DATA_ROOT），全部显式提供；它不进 CI。\n",
    );
    process.exit(1);
  }
  return value;
}

export function delay(ms: number): Promise<void> {
  return new Promise((resolvePromise) => setTimeout(resolvePromise, ms));
}

/** 从 env 写出 smoke 自己的 personal provider 配置（OpenAI Chat Completions）。 */
export async function writeSmokeProviderConfig(filePath: string, baseUrl: string, apiKey: string): Promise<void> {
  const config = {
    schemaVersion: 1,
    config: {
      providerConfigRules: {
        providerRules: [
          {
            providerId: PROVIDER_ID,
            providerName: "Agent Server Smoke",
            enabled: true,
            config: {
              group: "standard-personal",
              visibility: "visible",
              access: { type: "api-key", apiKey },
              personalModelIds: [process.env.MODE_SMOKE_MODEL],
              api: { type: "openai-chat-completions", baseUrl },
            },
          },
        ],
      },
      modelConfigRules: {
        providerModelRules: [
          {
            providerId: PROVIDER_ID,
            modelId: process.env.MODE_SMOKE_MODEL,
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
      providerOrder: [PROVIDER_ID],
    },
  };
  await writeFile(filePath, `${JSON.stringify(config, null, 2)}\n`, "utf8");
}

export type SmokeSession = {
  sessionId: string;
  request(method: string, params: unknown): Promise<unknown>;
  readonly frames: Frame[];
  readonly stderr: string;
  stop(): Promise<void>;
};

export async function startSmokeServer(dataRoot: string, cwd: string, configPath: string): Promise<SmokeSession> {
  const binary = resolveBinary();
  const frames: Frame[] = [];
  let stderrText = "";
  let requestSeq = 0;
  const pending = new Map<
    number,
    { resolve: (value: unknown) => void; reject: (error: Error) => void }
  >();
  let child: ChildProcess | undefined;
  const write = (message: unknown): void => {
    child?.stdin?.write(`${JSON.stringify(message)}\n`);
  };
  const request = async (method: string, params: unknown): Promise<unknown> => {
    requestSeq += 1;
    const id = requestSeq;
    const answer = new Promise<unknown>((resolvePromise, rejectPromise) => {
      pending.set(id, { resolve: resolvePromise, reject: rejectPromise });
    });
    write({ id, method, params });
    const timeout = new Promise<never>((_resolve, rejectPromise) => {
      const timer = setTimeout(() => {
        pending.delete(id);
        rejectPromise(new Error(`${method} 在 ${REQUEST_TIMEOUT_MS}ms 内没有应答`));
      }, REQUEST_TIMEOUT_MS);
      timer.unref?.();
    });
    return Promise.race([answer, timeout]);
  };

  const handleFrame = (frame: Record<string, unknown>): void => {
    const method = typeof frame.method === "string" ? frame.method : undefined;
    if (method && frame.id !== undefined) {
      if (method === "session/requestRuntimePreferences") {
        write({
          id: frame.id,
          result: {
            nativeSearchEnhancementsEnabled: false,
            memoryEnabled: false,
            askUserQuestionAutoResolutionEnabled: true,
            modelContextBudgetStrategy: "preflight-v1",
          },
        });
        return;
      }
      if (method === "interaction/requestPermission") {
        // Read 是只读工具，正常不会审批；显式放行兜底（与 oracle driver 同口径）。
        write({ id: frame.id, result: { decision: "allow" } });
        return;
      }
      write({
        id: frame.id,
        error: { code: -32601, message: `smoke driver 未处理 ${method}` },
      });
      return;
    }
    if (method === "session/event") {
      const params = (frame.params ?? {}) as Record<string, unknown>;
      frames.push({ type: String(params.type ?? ""), payload: params.payload });
      return;
    }
    if (frame.id !== undefined) {
      const id = Number(frame.id);
      const waiter = pending.get(id);
      if (!waiter) return;
      pending.delete(id);
      if (frame.error !== undefined) {
        waiter.reject(new Error(`请求 #${id} 失败：${JSON.stringify(frame.error)}`));
        return;
      }
      waiter.resolve(frame.result);
    }
  };

  // 不继承测试专用脚本 provider：smoke 必须打真实端点，不能被外层 env 静默替换。
  const childEnv: NodeJS.ProcessEnv = { ...process.env };
  delete childEnv.MODE_AGENT_SERVER_SCRIPTED_PROVIDER;
  child = spawn(binary, ["app-server", "--stdio"], {
    cwd: REPO_ROOT,
    env: {
      ...childEnv,
      MODE_DATA_BASE_DIR: dataRoot,
      MODE_HOME: dataRoot,
      HOME: dataRoot,
      USERPROFILE: dataRoot,
      MODE_PERSONAL_PROVIDER_CONFIG_FILE: configPath,
      MODE_BUILTIN_PROVIDER_CONFIG_FILE: join(REPO_ROOT, "config", "provider", "mode-builtin.json"),
    },
    stdio: ["pipe", "pipe", "pipe"],
  });
  child.stderr?.on("data", (chunk: Buffer) => {
    stderrText += chunk.toString();
  });

  // stdout 按行缓冲：`data` 块可以切在任意字节位置（包括一行 JSON 的中间）。
  let stdoutBuffer = "";
  let storageListener: ((frame: Record<string, unknown>) => void) | undefined;
  const consumeLine = (line: string): void => {
    if (!line.trim()) return;
    try {
      const frame = JSON.parse(line) as Record<string, unknown>;
      storageListener?.(frame);
      handleFrame(frame);
    } catch {
      fail(`agent-server stdout 出现解析失败行：${line}`);
    }
  };
  const consumeChunk = (chunk: Buffer): void => {
    stdoutBuffer += chunk.toString();
    const lines = stdoutBuffer.split("\n");
    stdoutBuffer = lines.pop() ?? "";
    for (const line of lines) consumeLine(line);
  };
  child.stdout?.on("data", consumeChunk);
  child.stdout?.on("end", () => {
    consumeLine(stdoutBuffer);
    stdoutBuffer = "";
  });

  const storageReady = new Promise<void>((resolvePromise, rejectPromise) => {
    const timer = setTimeout(() => {
      rejectPromise(new Error(`启动超时（storage ready 未到）。stderr=${stderrText}`));
    }, STARTUP_TIMEOUT_MS);
    timer.unref?.();
    storageListener = (frame: Record<string, unknown>): void => {
      if (frame.method !== "startup/storageState") return;
      const phase = (frame.params as Record<string, unknown> | undefined)?.phase;
      if (phase === "ready") {
        clearTimeout(timer);
        resolvePromise();
      }
      if (phase === "failed") {
        clearTimeout(timer);
        rejectPromise(new Error(`存储门 failed：stderr=${stderrText}`));
      }
    };
  });
  await storageReady;
  const model = String(process.env.MODE_SMOKE_MODEL);
  const created = (await request("session/create", {
    workspace: { workspacePath: cwd, workspaceKey: cwd },
    model: { providerId: PROVIDER_ID, modelId: model, options: { reasoningLevel: "disabled" } },
    thoughtLevel: "disabled",
    titleGenerationEnabled: false,
  })) as { session?: { sessionId?: string }; sessionId?: string } | undefined;
  const sessionId = created?.sessionId ?? created?.session?.sessionId;
  if (!sessionId) fail(`session/create 没有返回 sessionId：${JSON.stringify(created)}`);
  await request("session/subscribe", {
    sessionId,
    deliveryKind: "desktop-continuous",
    includeSnapshot: true,
  });
  return {
    sessionId,
    request,
    frames,
    get stderr() {
      return stderrText;
    },
    async stop() {
      for (const [id, waiter] of pending) {
        pending.delete(id);
        waiter.reject(new Error(`stop() 时请求 #${id} 仍未应答`));
      }
      child?.stdin?.end();
      const exitPromise = new Promise<void>((resolvePromise) => {
        child?.on("exit", () => resolvePromise());
      });
      const exited = await Promise.race([
        exitPromise.then(() => true),
        delay(5_000).then(() => false),
      ]);
      if (!exited) {
        child?.kill();
        await Promise.race([exitPromise, delay(2_000)]);
      }
    },
  };
}

function resolveBinary(): string {
  const configured = process.env.MODE_AGENT_SERVER_COMMAND?.trim();
  if (configured) return configured;
  for (const profile of ["debug", "release"]) {
    for (const name of ["agent-server", "mode-agent"]) {
      for (const suffix of [".exe", ""]) {
        const candidate = join(REPO_ROOT, "target", profile, `${name}${suffix}`);
        if (existsSync(candidate)) return candidate;
      }
    }
  }
  fail(
    "agent-server 二进制不存在（target/{debug,release}/{agent-server,mode-agent}）：" +
      "先跑 `cargo build -p agent-server`，或用 MODE_AGENT_SERVER_COMMAND 指定路径",
  );
}

export async function waitForTerminal(session: SmokeSession, baseline: number): Promise<Terminal> {
  const deadline = Date.now() + TURN_TIMEOUT_MS;
  const terminals = (): Frame[] =>
    session.frames.filter(
      (frame) => frame.type === "turn.completed" || frame.type === "turn.failed",
    );
  while (terminals().length <= baseline) {
    if (Date.now() > deadline) {
      fail(
        `turn 在 ${TURN_TIMEOUT_MS}ms 内没有终态。已收到事件：` +
          `${session.frames.map((frame) => frame.type).join(",")}\nstderr=${session.stderr}`,
      );
    }
    await delay(50);
  }
  const terminal = terminals().at(-1) as Frame;
  const payload = (terminal.payload ?? {}) as Record<string, unknown>;
  if (terminal.type === "turn.failed") {
    fail(`turn.failed：${JSON.stringify(payload)}\nstderr=${session.stderr}`);
  }
  return { kind: "completed", payload };
}

export function usageOf(terminal: Terminal): Record<string, number> {
  const usage = (terminal.payload.usage ?? {}) as Record<string, unknown>;
  const number = (key: string): number =>
    typeof usage[key] === "number" ? (usage[key] as number) : -1;
  return {
    modelRequestCount: number("modelRequestCount"),
    inputTokens: number("inputTokens"),
    outputTokens: number("outputTokens"),
    totalTokens: number("totalTokens"),
  };
}
