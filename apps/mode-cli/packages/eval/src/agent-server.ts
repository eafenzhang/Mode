// agent-server stdio 驱动（spec §2）：起进程、NDJSON 收发、反向请求默认应答、事件收集。
// 帧语义与 oracle 驱动（packages/desktop/tests/agent-runtime-oracle）一致：
// NDJSON 四类帧（通知 / 反向请求 / 应答 / 错误应答），`session/event` 映射为 {type, payload}。
import { spawn, type ChildProcess } from "node:child_process";

import type { EvalCase } from "./types.js";

export type EvalSessionEvent = { type: string; payload: unknown };

export type EvalSession = {
  /** 提交一次用户输入并等待该 turn 的终态事件（`turn.completed`/`turn.failed`）。 */
  send(prompt: string): Promise<void>;
  readonly events: readonly EvalSessionEvent[];
  readonly stderr: string;
  readonly invalidLines: readonly string[];
  stop(): Promise<void>;
};

export type SessionStartInput = {
  case: EvalCase;
  /** 本 case 的脚本夹具路径（写进 `MODE_AGENT_SERVER_SCRIPTED_PROVIDER`）。 */
  scriptPath: string;
  /** case workspace（agent-server 的 cwd 与 `workspace.workspacePath`）。 */
  cwd: string;
  /** 隔离数据根（`MODE_DATA_BASE_DIR`/`HOME`/`USERPROFILE`）。 */
  dataRoot: string;
  timeoutMs: number;
};

export type SessionFactory = {
  start(input: SessionStartInput): Promise<EvalSession>;
};

export type AgentServerLaunch = {
  command: string;
  args: string[];
};

const REQUEST_TIMEOUT_MS = 60_000;
const TERMINAL_EVENT_TYPES: ReadonlySet<string> = new Set(["turn.completed", "turn.failed"]);
const REVERSE_REQUEST_TIMEOUT_MS = 60_000;

/** 与 oracle 驱动相同的宿主默认应答表。 */
const RUNTIME_PREFERENCES_RESULT = {
  nativeSearchEnhancementsEnabled: false,
  memoryEnabled: false,
  askUserQuestionAutoResolutionEnabled: true,
  modelContextBudgetStrategy: "preflight-v1",
} as const;

export function createAgentServerSessionFactory(
  launch: AgentServerLaunch,
  baseEnv: NodeJS.ProcessEnv = process.env,
): SessionFactory {
  return {
    async start(input: SessionStartInput): Promise<EvalSession> {
      return startAgentServerSession(launch, baseEnv, input);
    },
  };
}

async function startAgentServerSession(
  launch: AgentServerLaunch,
  baseEnv: NodeJS.ProcessEnv,
  input: SessionStartInput,
): Promise<EvalSession> {
  const events: EvalSessionEvent[] = [];
  const invalidLines: string[] = [];
  const frames: Record<string, unknown>[] = [];
  const pending = new Map<number, { resolve: (value: unknown) => void; reject: (error: Error) => void }>();
  const reverseRequests: { method: string; params: unknown }[] = [];
  const terminalWaiters: Array<{ resolve: () => void }> = [];
  let stderrText = "";
  let requestSeq = 0;
  let stopped = false;
  let child: ChildProcess | undefined;

  const write = (message: unknown): void => {
    try {
      child?.stdin?.write(`${JSON.stringify(message)}\n`);
    } catch {
      // 子进程已退出（EPIPE）：不再写；stop() 会收口。
    }
  };

  const request = (method: string, params: unknown, timeoutMs = REQUEST_TIMEOUT_MS): Promise<unknown> => {
    requestSeq += 1;
    const id = requestSeq;
    const answer = new Promise<unknown>((resolvePromise, rejectPromise) => {
      pending.set(id, { resolve: resolvePromise, reject: rejectPromise });
    });
    const timeout = new Promise<never>((_resolve, rejectPromise) => {
      const timer = setTimeout(() => {
        pending.delete(id);
        rejectPromise(new Error(`eval：${method} 在 ${timeoutMs}ms 内没有应答`));
      }, timeoutMs);
      timer.unref?.();
    });
    write({ id, method, params });
    return Promise.race([answer, timeout]);
  };

  const answerReverse = (method: string, frame: Record<string, unknown>): boolean => {
    if (method === "session/requestRuntimePreferences") {
      write({ id: frame.id, result: RUNTIME_PREFERENCES_RESULT });
      return true;
    }
    if (method === "interaction/requestPermission") {
      // 评测默认放行（与 oracle 驱动同口径）；审批三态不在首片范围。
      write({ id: frame.id, result: { decision: "allow" } });
      return true;
    }
    return false;
  };

  const handleFrame = (frame: Record<string, unknown>): void => {
    frames.push(frame);
    const method = typeof frame.method === "string" ? frame.method : undefined;
    if (method && frame.id !== undefined) {
      reverseRequests.push({ method, params: frame.params });
      if (!answerReverse(method, frame)) {
        write({ id: frame.id, error: { code: -32601, message: `eval driver 未处理 ${method}` } });
      }
      return;
    }
    if (method === "session/event") {
      const params = (frame.params ?? {}) as Record<string, unknown>;
      const event: EvalSessionEvent = {
        type: String(params.type ?? "session/event"),
        payload: params.payload,
      };
      events.push(event);
      if (TERMINAL_EVENT_TYPES.has(event.type)) {
        for (const waiter of terminalWaiters.splice(0)) waiter.resolve();
      }
      return;
    }
    if (frame.id !== undefined) {
      const id = Number(frame.id);
      const waiter = pending.get(id);
      if (!waiter) return;
      pending.delete(id);
      if (frame.error !== undefined) {
        waiter.reject(new Error(`eval：请求 #${id} 失败：${JSON.stringify(frame.error)}`));
        return;
      }
      waiter.resolve(frame.result);
    }
  };

  child = spawn(launch.command, launch.args, {
    cwd: input.cwd,
    env: {
      ...baseEnv,
      // 隔离纪律：数据树与 home 都指向隔离根；脚本 provider 从本 case 夹具出流。
      MODE_DATA_BASE_DIR: input.dataRoot,
      MODE_HOME: input.dataRoot,
      HOME: input.dataRoot,
      USERPROFILE: input.dataRoot,
      MODE_AGENT_SERVER_SCRIPTED_PROVIDER: input.scriptPath,
      MODE_PERSONAL_PROVIDER_CONFIG_FILE: `${input.dataRoot}/eval-no-personal-provider.json`,
      MODE_BUILTIN_PROVIDER_CONFIG_FILE: `${input.dataRoot}/eval-no-builtin-provider.json`,
    },
    stdio: ["pipe", "pipe", "pipe"],
  });

  let stdoutBuffer = "";
  child.stdout?.on("data", (chunk: Buffer) => {
    stdoutBuffer += chunk.toString();
    let newlineIndex = stdoutBuffer.indexOf("\n");
    while (newlineIndex >= 0) {
      const line = stdoutBuffer.slice(0, newlineIndex);
      stdoutBuffer = stdoutBuffer.slice(newlineIndex + 1);
      newlineIndex = stdoutBuffer.indexOf("\n");
      if (!line.trim()) continue;
      let parsed: Record<string, unknown>;
      try {
        parsed = JSON.parse(line) as Record<string, unknown>;
      } catch {
        invalidLines.push(line);
        continue;
      }
      handleFrame(parsed);
    }
  });
  child.stderr?.on("data", (chunk: Buffer) => {
    stderrText += chunk.toString();
  });

  const exitPromise = new Promise<void>((resolvePromise) => {
    child?.on("exit", () => resolvePromise());
  });

  const stop = async (): Promise<void> => {
    if (stopped) return;
    stopped = true;
    for (const [id, waiter] of pending) {
      pending.delete(id);
      waiter.reject(new Error(`eval：stop() 时请求 #${id} 仍未应答`));
    }
    child?.stdin?.end();
    const exited = await Promise.race([
      exitPromise.then(() => true),
      delay(5_000).then(() => false),
    ]);
    if (exited) return;
    child?.kill();
    const killed = await Promise.race([
      exitPromise.then(() => true),
      delay(2_000).then(() => false),
    ]);
    if (!killed) {
      child?.kill("SIGKILL");
      await exitPromise;
    }
  };

  const waitForTerminal = (timeoutMs: number): Promise<void> =>
    new Promise((resolvePromise, rejectPromise) => {
      // 事件可能已经先到：先查一遍再挂 waiter，避免竞态漏掉终态。
      if (events.some((event) => TERMINAL_EVENT_TYPES.has(event.type))) {
        resolvePromise();
        return;
      }
      const timer = setTimeout(() => {
        rejectPromise(
          new Error(
            `eval：turn 在 ${timeoutMs}ms 内没有终态事件；stderr=${stderrText.slice(0, 2000)}`,
          ),
        );
      }, timeoutMs);
      timer.unref?.();
      terminalWaiters.push({
        resolve: () => {
          clearTimeout(timer);
          resolvePromise();
        },
      });
    });

  try {
    await waitForStorageReady(child, invalidLines, () => stderrText, input.timeoutMs);
    const created = (await request("session/create", {
      workspace: { workspacePath: input.cwd, workspaceKey: input.cwd },
      model: {
        providerId: "oracle-mock",
        modelId: "mock-1",
        options: { reasoningLevel: "disabled" },
      },
      thoughtLevel: "disabled",
      // 标题生成是一次额外模型调用，会让事件流尾部顺序不稳定；与 oracle 对拍同款关闭。
      titleGenerationEnabled: false,
    })) as { sessionId?: string; session?: { sessionId?: string } } | undefined;
    const sessionId = created?.sessionId ?? created?.session?.sessionId;
    if (!sessionId) {
      throw new Error(`eval：session/create 没有返回 sessionId：${JSON.stringify(created)}`);
    }
    await request("session/subscribe", {
      sessionId,
      deliveryKind: "desktop-continuous",
      includeSnapshot: true,
    });

    const send = async (prompt: string): Promise<void> => {
      const result = (await request("session/send", { sessionId, content: prompt })) as
        | { accepted?: boolean }
        | undefined;
      if (result?.accepted !== true) {
        throw new Error(`eval：session/send 未被受理：${JSON.stringify(result)}`);
      }
      await waitForTerminal(input.timeoutMs);
    };

    return {
      send,
      events,
      get stderr() {
        return stderrText;
      },
      invalidLines,
      stop,
    };
  } catch (error) {
    await stop();
    throw error;
  }
}

async function waitForStorageReady(
  child: ChildProcess,
  invalidLines: readonly string[],
  readStderr: () => string,
  timeoutMs: number,
): Promise<void> {
  return new Promise<void>((resolvePromise, rejectPromise) => {
    let settled = false;
    const timer = setTimeout(() => {
      if (settled) return;
      settled = true;
      rejectPromise(
        new Error(
          `eval：agent-server 启动超时（${timeoutMs}ms 内没有 storage ready）；` +
            `stderr=${readStderr().slice(0, 2000)}`,
        ),
      );
    }, timeoutMs);
    timer.unref?.();

    const check = (frame: Record<string, unknown>): void => {
      if (frame.method !== "startup/storageState") return;
      const params = (frame.params ?? {}) as Record<string, unknown>;
      const phase = String(params.phase ?? "");
      if (phase === "ready" && !settled) {
        settled = true;
        clearTimeout(timer);
        resolvePromise();
      }
      if (phase === "failed" && !settled) {
        settled = true;
        clearTimeout(timer);
        rejectPromise(new Error(`eval：agent-server 存储门失败：${JSON.stringify(params)}`));
      }
    };

    let buffer = "";
    child.stdout?.on("data", (chunk: Buffer) => {
      buffer += chunk.toString();
      let newlineIndex = buffer.indexOf("\n");
      while (newlineIndex >= 0) {
        const line = buffer.slice(0, newlineIndex);
        buffer = buffer.slice(newlineIndex + 1);
        newlineIndex = buffer.indexOf("\n");
        if (!line.trim()) continue;
        let parsed: Record<string, unknown>;
        try {
          parsed = JSON.parse(line) as Record<string, unknown>;
        } catch {
          continue;
        }
        check(parsed);
      }
    });
    child.on("exit", () => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      rejectPromise(
        new Error(
          `eval：agent-server 在存储门就绪前退出；invalidLines=${JSON.stringify(invalidLines.slice(0, 5))} ` +
            `stderr=${readStderr().slice(0, 2000)}`,
        ),
      );
    });
  });
}

function delay(ms: number): Promise<void> {
  return new Promise((resolvePromise) => {
    setTimeout(resolvePromise, ms);
  });
}

export { REVERSE_REQUEST_TIMEOUT_MS };
