// C1 Task 10：Rust `agent-server` 的 stdio 会话驱动（回放 harness 的执行半边）。
//
// 职责只有「跑会话、收帧、答反向请求」：NDJSON 四类帧（与 `driver.mts` 的 oracle 驱动
// 同一帧语义）、隔离纪律（`assertDataRootIsolated` + HOME/USERPROFILE/MODE_HOME 覆盖）、
// 启动时预建**已迁移**基线库（`agent-server-env.mts`）、反向请求按宿主默认口径应答
// （偏好 → spec §1.3 默认表；权限 → allow）。**不实现任何比较规则**——投影与比较在
// `normalize.mts` / `compare.mts`，回放编排在 `replay-agent-server.mts`。
import { spawn, type ChildProcess } from "node:child_process";
import { join } from "node:path";

import { assertDataRootIsolated } from "./driver.mjs";
import {
  REPO_ROOT,
  createBaselineDatabase,
  ensureAgentServerBinary,
} from "./agent-server-env.mjs";

/** 与 `driver.mts` 相同的偏好应答（spec §1.3 的宿主默认应答表）。 */
const RUNTIME_PREFERENCES_RESULT = {
  nativeSearchEnhancementsEnabled: false,
  memoryEnabled: false,
  askUserQuestionAutoResolutionEnabled: true,
  modelContextBudgetStrategy: "preflight-v1",
} as const;

const REQUEST_TIMEOUT_MS = 60_000;
const STARTUP_TIMEOUT_MS = 60_000;

/** C1 会话模型：provider/model 事实只进 create 参数（脚本化装配源忽略它们并按夹具剧本出流）。 */
const REPLAY_PROVIDER_ID = "oracle-mock";
const REPLAY_MODEL_ID = "mock-1";

export type AgentServerEvent = { type: string; payload: unknown };

export type AgentServerSession = {
  readonly sessionId: string;
  /**
   * 发一条任意请求并等应答；**仅供方法序回放（probe）与诊断**，三层回放不直接用
   * （直接发请求会绕过 `send()` 的「受理」断言）。
   */
  request(method: string, params: unknown): Promise<unknown>;
  send(content: string): Promise<void>;
  readonly frames: AgentServerEvent[];
  readonly reverseRequests: { method: string; params: unknown }[];
  readonly storageGate: { phase: string; errorCode?: string }[];
  readonly invalidLines: string[];
  readonly stderr: string;
  stop(): Promise<void>;
};

export type StartOptions = {
  /** 隔离数据根（mkdtemp；三层回放必须用 `mode-oracle-fixture-` 前缀，见 replay 文件头）。 */
  dataRoot: string;
  /** 会话工作目录（三层回放必须用 `mode-oracle-fixture-ws-` 前缀）。 */
  cwd: string;
  /**
   * oracle 行为夹具路径（`MODE_AGENT_SERVER_SCRIPTED_PROVIDER` 的值）。
   * 省略 = 显式清空该变量（真实 hub 装配；`probe-session-lifecycle --replay` 的
   * 方法序回放自行写临时夹具传入）。
   */
  fixturePath?: string;
};

/** 回放驱动器：起 Rust 二进制、答反向请求、收帧；不实现任何比较规则。 */
export async function startAgentServer(options: StartOptions): Promise<AgentServerSession> {
  const dataRoot = options.dataRoot;
  const cwd = options.cwd;
  assertDataRootIsolated(dataRoot);
  await createBaselineDatabase(dataRoot);

  const binary = ensureAgentServerBinary();
  const frames: AgentServerEvent[] = [];
  const reverseRequests: { method: string; params: unknown }[] = [];
  const storageGate: { phase: string; errorCode?: string }[] = [];
  const invalidLines: string[] = [];
  let stderrText = "";
  let requestSeq = 0;
  const pending = new Map<
    number,
    { resolve: (value: unknown) => void; reject: (error: Error) => void }
  >();
  let child: ChildProcess | undefined;
  let stopped = false;

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
        rejectPromise(new Error(`replay：${method} 在 ${REQUEST_TIMEOUT_MS}ms 内没有应答`));
      }, REQUEST_TIMEOUT_MS);
      timer.unref?.();
    });
    return Promise.race([answer, timeout]);
  };

  /** 反向请求应答（宿主职责的一半；口径与 `driver.mts` 相同）。 */
  const answerReverse = (method: string, frame: Record<string, unknown>): boolean => {
    if (method === "session/requestRuntimePreferences") {
      write({ id: frame.id, result: RUNTIME_PREFERENCES_RESULT });
      return true;
    }
    if (method === "interaction/requestPermission") {
      // 默认放行：夹具录制时 driver 也是默认放行（审批三态不在这批夹具里）。
      write({ id: frame.id, result: { decision: "allow" } });
      return true;
    }
    return false;
  };

  const handleFrame = (frame: Record<string, unknown>): void => {
    const method = typeof frame.method === "string" ? frame.method : undefined;
    if (method && frame.id !== undefined) {
      reverseRequests.push({ method, params: frame.params });
      if (!answerReverse(method, frame)) {
        // 未登记的反向请求显式回错，不静默忽略（与 driver.mts 的同一纪律）。
        write({
          id: frame.id,
          error: { code: -32601, message: `replay driver 未处理 ${method}` },
        });
      }
      return;
    }
    if (method === "session/event") {
      const params = (frame.params ?? {}) as Record<string, unknown>;
      frames.push({ type: String(params.type ?? "session/event"), payload: params.payload });
      return;
    }
    if (method === "startup/storageState") {
      const params = (frame.params ?? {}) as Record<string, unknown>;
      storageGate.push({
        phase: String(params.phase ?? ""),
        errorCode: typeof params.errorCode === "string" ? params.errorCode : undefined,
      });
      return;
    }
    if (frame.id !== undefined) {
      const id = Number(frame.id);
      const waiter = pending.get(id);
      if (!waiter) return;
      pending.delete(id);
      if (frame.error !== undefined) {
        waiter.reject(new Error(`replay：请求 #${id} 失败：${JSON.stringify(frame.error)}`));
        return;
      }
      waiter.resolve(frame.result);
    }
  };

  child = spawn(binary, ["app-server", "--stdio"], {
    cwd: REPO_ROOT,
    env: {
      ...process.env,
      // 隔离纪律（driver.mts 同款）：数据树与 CLI 布局都指向隔离根。
      MODE_DATA_BASE_DIR: dataRoot,
      MODE_HOME: dataRoot,
      HOME: dataRoot,
      USERPROFILE: dataRoot,
      // 测试专用开关：scripted provider 从夹具 `script` 出流，生产路径不存在。
      // 未提供时该变量显式清空（from_env 对空白视同未设置）。
      MODE_AGENT_SERVER_SCRIPTED_PROVIDER: options.fixturePath ?? "",
      // 开发机上这两个变量指向真实凭据文件，必须屏蔽（scripted 装配不用它们，
      // 但能力解析会读 hub——屏蔽只影响能力数字，不产生任何真实端点访问）。
      MODE_PERSONAL_PROVIDER_CONFIG_FILE: join(dataRoot, "replay-no-personal-provider.json"),
      MODE_BUILTIN_PROVIDER_CONFIG_FILE: join(dataRoot, "replay-no-builtin-provider.json"),
    },
    stdio: ["pipe", "pipe", "pipe"],
  });

  let stdoutBuffer = "";
  const storageReady = new Promise<void>((resolvePromise, rejectPromise) => {
    let settled = false;
    const timer = setTimeout(() => {
      if (!settled) {
        settled = true;
        rejectPromise(
          new Error(
            `replay：agent-server 启动超时（${STARTUP_TIMEOUT_MS}ms 内没有 storage ready）。` +
              `gate=${JSON.stringify(storageGate)} stderr=${stderrText}`,
          ),
        );
      }
    }, STARTUP_TIMEOUT_MS);
    timer.unref?.();
    const check = (frame: Record<string, unknown>): void => {
      if (frame.method !== "startup/storageState") return;
      const phase = (frame.params as Record<string, unknown> | undefined)?.phase;
      if (phase === "ready" && !settled) {
        settled = true;
        clearTimeout(timer);
        resolvePromise();
      }
      if (phase === "failed" && !settled) {
        settled = true;
        clearTimeout(timer);
        rejectPromise(
          new Error(
            `replay：agent-server 存储门失败：${JSON.stringify(storageGate)} stderr=${stderrText}`,
          ),
        );
      }
    };
    child?.stdout?.on("data", (chunk: Buffer) => {
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
        check(parsed);
        handleFrame(parsed);
      }
    });
  });

  child.stderr?.on("data", (chunk: Buffer) => {
    stderrText += chunk.toString();
  });
  const exitPromise = new Promise<void>((resolvePromise) => {
    child?.on("exit", () => resolvePromise());
  });

  try {
    await storageReady;
    const created = (await request("session/create", {
      workspace: { workspacePath: cwd, workspaceKey: cwd },
      model: {
        providerId: REPLAY_PROVIDER_ID,
        modelId: REPLAY_MODEL_ID,
        options: { reasoningLevel: "disabled" },
      },
      thoughtLevel: "disabled",
      // 标题生成不在 C1 事件面（差异已登记）；与 oracle 对拍同款关闭。
      titleGenerationEnabled: false,
    })) as { sessionId?: string; session?: { sessionId?: string } } | undefined;
    const sessionId = created?.sessionId ?? created?.session?.sessionId;
    if (!sessionId) {
      throw new Error(`replay：session/create 没有返回 sessionId：${JSON.stringify(created)}`);
    }
    await request("session/subscribe", {
      sessionId,
      deliveryKind: "desktop-continuous",
      includeSnapshot: true,
    });
    return {
      sessionId,
      request,
      async send(content: string) {
        const result = (await request("session/send", { sessionId, content })) as
          | { accepted?: boolean }
          | undefined;
        if (result?.accepted !== true) {
          throw new Error(`replay：session/send 未被受理：${JSON.stringify(result)}`);
        }
      },
      frames,
      reverseRequests,
      storageGate,
      invalidLines,
      get stderr() {
        return stderrText;
      },
      async stop() {
        if (stopped) return;
        stopped = true;
        for (const [id, waiter] of pending) {
          pending.delete(id);
          waiter.reject(new Error(`replay：stop() 时请求 #${id} 仍未应答`));
        }
        child?.stdin?.end();
        const exited = await Promise.race([
          exitPromise.then(() => true),
          delay(5_000).then(() => false),
        ]);
        if (!exited) {
          child?.kill();
          const killed = await Promise.race([
            exitPromise.then(() => true),
            delay(2_000).then(() => false),
          ]);
          if (!killed) {
            child?.kill("SIGKILL");
            await exitPromise;
          }
        }
      },
    };
  } catch (error) {
    child?.kill();
    throw error;
  }
}

function delay(ms: number): Promise<void> {
  return new Promise((resolvePromise) => {
    setTimeout(resolvePromise, ms);
  });
}
