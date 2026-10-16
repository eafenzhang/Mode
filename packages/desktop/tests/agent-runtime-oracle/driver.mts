// M2 Task 0：行为对拍 oracle 驱动（TS 侧，唯一一份）。
//
// 规格：docs/specs/tauri-agent-runtime.md §1（oracle 契约）、§3（合法差异）、§4（mock provider）。
// 纪律：docs/specs/clean-room-policy.md —— 本模块只**运行**旧实现并观察输入输出与事件序列，
// 不读它的实现体。
//
// 线上事实（2026-10-16 实测，spec §1.2）：stdio 是 NDJSON，帧形状是 JSON-RPC 风格的四类
// （通知 / 反向请求 / 应答 / 错误应答），**不是** `{type, payload}`；
// `OracleEvent` 是本驱动层的映射，其中 `session/event` 通知映射成
// `{type: params.type, payload: params.payload}`，其他方法映射成 `{type: method, payload: params}`。
import { spawn, type ChildProcess } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

import {
  ORACLE_MOCK_MODEL_ID,
  ORACLE_MOCK_PROVIDER_ID,
  startMockProvider,
  substituteCwd,
  writeOracleProviderConfig,
  type MockStep,
} from "./mock-provider.mjs";

const REPO_ROOT = fileURLToPath(new URL("../../../../", import.meta.url));
const TSX_CLI = join(REPO_ROOT, "node_modules", "tsx", "dist", "cli.mjs");
const CLI_ENTRY = join(REPO_ROOT, "apps", "mode-cli", "packages", "cli", "src", "main.ts");
const BUILTIN_PROVIDER_CONFIG = join(REPO_ROOT, "config", "provider", "mode-builtin.json");

/** 会话事件（或任一帧）在驱动层的视图；`type` 的映射规则见文件头。 */
export type OracleEvent = { type: string; payload: unknown };

export type OracleSession = {
  readonly sessionId: string;
  /** 提交一次用户输入并等待 CLI 受理（不等 turn 结束——turn 的终态从事件流里读）。 */
  send(prompt: string): Promise<void>;
  /** 会话事件流；`stop()` 后关闭。支持 `for await … break`。 */
  events: AsyncIterable<OracleEvent>;
  /** 收到的所有帧（含启动帧与遥测帧），仅供诊断与断言。 */
  readonly frames: OracleEvent[];
  /** 解析失败的 stdout 行：**不静默丢**，聚合在这里。 */
  readonly invalidLines: string[];
  /** 子进程 stderr 全文。 */
  readonly stderr: string;
  /** mock provider 收到的请求路径，用来证明流量没打到真实模型端点。 */
  readonly mockRequestPaths: readonly string[];
  stop(): Promise<void>;
};

export type OracleRuntimeOptions = {
  /** 隔离数据根：必须是 mkdtemp 出来的目录，不能是用户真实 `~/.mode`。 */
  dataRoot: string;
  /** 工作目录（session 的 workspacePath/workspaceKey 都用它）。 */
  cwd: string;
  /** mock provider 的剧本；不传用内置默认剧本，见 DEFAULT_SCRIPT。 */
  script?: MockStep[];
};

/**
 * 不传剧本时的默认剧本：驱动自检用例只验「起得来、吐得出事件、退得干净」，
 * 需要一个确定的工具调用与终态文本。夹具一律显式传剧本（spec §3.1）。
 */
const DEFAULT_SCRIPT: MockStep[] = [
  { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/hello.txt" } }] },
  { text: "oracle-mock-answer" },
];

/** 反向请求的应答表（spec §1.3）。未列出的显式回错误，不静默忽略。 */
const RUNTIME_PREFERENCES_RESULT = {
  nativeSearchEnhancementsEnabled: false,
  memoryEnabled: false,
  askUserQuestionAutoResolutionEnabled: true,
  modelContextBudgetStrategy: "preflight-v1",
} as const;

const REQUEST_TIMEOUT_MS = 60_000;
const STARTUP_TIMEOUT_MS = 60_000;

/** 数据根不能落在用户真实数据根上——对拍最贵的错误就是污染生产数据（spec §1.4）。 */
export function assertDataRootIsolated(dataRoot: string): void {
  const resolved = resolve(dataRoot);
  for (const forbidden of [join(homedir(), ".mode"), join(homedir(), ".mode")]) {
    if (resolved === forbidden || resolved.startsWith(`${forbidden}${sep}`)) {
      throw new Error(`对拍数据根指向用户真实数据根：${resolved}`);
    }
  }
}

function createEventQueue(): {
  push(event: OracleEvent): void;
  close(): void;
  iterable: AsyncIterable<OracleEvent>;
} {
  const items: OracleEvent[] = [];
  const waiters: ((result: IteratorResult<OracleEvent>) => void)[] = [];
  let closed = false;
  const done = (): IteratorResult<OracleEvent> => ({ value: undefined as never, done: true });
  return {
    push(event) {
      if (closed) return;
      const waiter = waiters.shift();
      if (waiter) {
        waiter({ value: event, done: false });
        return;
      }
      items.push(event);
    },
    close() {
      closed = true;
      while (waiters.length > 0) {
        waiters.shift()?.(done());
      }
    },
    iterable: {
      [Symbol.asyncIterator]() {
        return {
          next() {
            const item = items.shift();
            if (item) return Promise.resolve({ value: item, done: false });
            if (closed) return Promise.resolve(done());
            return new Promise<IteratorResult<OracleEvent>>((resolvePromise) => {
              waiters.push(resolvePromise);
            });
          },
          // `for await … break` 会调用它；这里只结束本次迭代，不结束会话。
          return: () => Promise.resolve(done()),
        };
      },
    },
  };
}

export async function startOracleRuntime(options: OracleRuntimeOptions): Promise<OracleSession> {
  const dataRoot = resolve(options.dataRoot);
  const cwd = resolve(options.cwd);
  assertDataRootIsolated(dataRoot);

  await mkdir(dataRoot, { recursive: true });
  const mockPersonalConfigPath = join(dataRoot, "oracle-personal-provider.json");
  const mock = await startMockProvider(substituteCwd(options.script ?? DEFAULT_SCRIPT, cwd));
  await writeOracleProviderConfig(mockPersonalConfigPath, mock.baseUrl);

  const frames: OracleEvent[] = [];
  const invalidLines: string[] = [];
  const queue = createEventQueue();
  let stderrText = "";
  let child: ChildProcess | undefined;
  let stopped = false;
  let requestSeq = 0;
  const pending = new Map<
    number,
    { resolve: (value: unknown) => void; reject: (error: Error) => void }
  >();

  const write = (message: unknown): void => {
    if (child?.stdin?.writable) {
      child.stdin.write(`${JSON.stringify(message)}\n`);
    }
  };

  /** 发一条请求并等应答；超时显式报错，不用「等一会儿」掩盖。 */
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
        rejectPromise(new Error(`oracle：${method} 在 ${REQUEST_TIMEOUT_MS}ms 内没有应答`));
      }, REQUEST_TIMEOUT_MS);
      timer.unref?.();
    });
    return Promise.race([answer, timeout]);
  };

  const handleInbound = (frame: Record<string, unknown>): void => {
    const method = String(frame.method);
    if (method === "session/requestRuntimePreferences") {
      write({ id: frame.id, result: RUNTIME_PREFERENCES_RESULT });
      return;
    }
    if (method === "interaction/requestPermission") {
      // Task 0 不测审批行为（审批三态在 Task 3 单独对拍），一律放行。
      write({ id: frame.id, result: { decision: "allow" } });
      return;
    }
    write({ id: frame.id, error: { code: -32601, message: `oracle driver 未处理 ${method}` } });
  };

  const handleFrame = (frame: Record<string, unknown>): void => {
    if (typeof frame.method === "string" && frame.id !== undefined) {
      handleInbound(frame);
      return;
    }
    if (typeof frame.method === "string") {
      const params = frame.params as Record<string, unknown> | undefined;
      const event: OracleEvent =
        frame.method === "session/event"
          ? { type: String(params?.type ?? "session/event"), payload: params?.payload }
          : { type: frame.method, payload: frame.params };
      frames.push(event);
      queue.push(event);
      return;
    }
    if (frame.id !== undefined) {
      const id = Number(frame.id);
      const waiter = pending.get(id);
      if (!waiter) return;
      pending.delete(id);
      if (frame.error !== undefined) {
        waiter.reject(new Error(`oracle：请求 #${id} 失败：${JSON.stringify(frame.error)}`));
        return;
      }
      waiter.resolve(frame.result);
    }
  };

  child = spawn(process.execPath, [TSX_CLI, CLI_ENTRY, "app-server", "--stdio"], {
    cwd: REPO_ROOT,
    env: {
      ...process.env,
      // 版本化数据树（v2）走 MODE_DATA_BASE_DIR；CLI 布局（cli/db、cli/log…）走 HOME。
      // 两者都要，否则对拍仍会写用户真实 ~/.mode/cli（spec §1.4 的实测事实）。
      MODE_DATA_BASE_DIR: dataRoot,
      MODE_HOME: dataRoot,
      HOME: dataRoot,
      USERPROFILE: dataRoot,
      // 必须覆盖：开发机上这两个变量分别指向真实凭据文件与已安装应用的配置目录。
      MODE_PERSONAL_PROVIDER_CONFIG_FILE: mockPersonalConfigPath,
      MODE_BUILTIN_PROVIDER_CONFIG_FILE: BUILTIN_PROVIDER_CONFIG,
    },
    stdio: ["pipe", "pipe", "pipe"],
  });

  let stdoutBuffer = "";
  const waitForStorageReady = (() => {
    let resolveReady: () => void = () => {};
    let ready = false;
    const promise = new Promise<void>((resolvePromise) => {
      resolveReady = resolvePromise;
    });
    return {
      promise,
      onFrame(frame: Record<string, unknown>): void {
        if (ready || frame.method !== "startup/storageState") return;
        const phase = (frame.params as Record<string, unknown> | undefined)?.phase;
        if (phase === "ready") {
          ready = true;
          resolveReady();
        }
      },
    };
  })();

  child.stdout?.on("data", (chunk: Buffer) => {
    stdoutBuffer += chunk.toString();
    let newlineIndex = stdoutBuffer.indexOf("\n");
    while (newlineIndex >= 0) {
      const line = stdoutBuffer.slice(0, newlineIndex);
      stdoutBuffer = stdoutBuffer.slice(newlineIndex + 1);
      newlineIndex = stdoutBuffer.indexOf("\n");
      if (!line.trim()) continue;
      let frame: Record<string, unknown>;
      try {
        frame = JSON.parse(line) as Record<string, unknown>;
      } catch {
        // 解析失败的行进 invalidLines 汇总，不静默丢（spec §1.2 的驱动纪律）。
        invalidLines.push(line);
        continue;
      }
      waitForStorageReady.onFrame(frame);
      handleFrame(frame);
    }
  });
  child.stderr?.on("data", (chunk: Buffer) => {
    stderrText += chunk.toString();
  });

  const exitPromise = new Promise<void>((resolvePromise) => {
    child?.on("exit", () => {
      resolvePromise();
    });
  });

  try {
    await withTimeout(
      waitForStorageReady.promise,
      STARTUP_TIMEOUT_MS,
      "oracle 启动（storage ready）",
    );
    // 归属文件是「本次进程的数据目录就是隔离根」的证据（docs/specs/mode-data-root.md）。
    const manifestPath = join(dataRoot, ".mode", ".mode-root.json");
    if (!existsSync(manifestPath)) {
      throw new Error(`oracle 未在隔离根写下归属文件：${manifestPath}；数据根隔离没有生效`);
    }

    const created = (await request("session/create", {
      workspace: { workspacePath: cwd, workspaceKey: cwd },
      model: {
        providerId: ORACLE_MOCK_PROVIDER_ID,
        modelId: ORACLE_MOCK_MODEL_ID,
        options: { reasoningLevel: "disabled" },
      },
      thoughtLevel: "disabled",
      // 标题生成是一次额外的模型调用，它的到达时刻不确定，会让事件流尾部顺序不稳定；
      // 它与被测行为（会话循环 / 工具执行）无关，对拍一律关掉。
      titleGenerationEnabled: false,
    })) as { sessionId?: string; session?: { sessionId?: string } } | undefined;
    // 实测形状：sessionId 嵌在 `session.sessionId`（顶层没有）。
    const sessionId = created?.sessionId ?? created?.session?.sessionId;
    if (!sessionId) {
      throw new Error(`oracle：session/create 没有返回 sessionId：${JSON.stringify(created)}`);
    }
    await request("session/subscribe", {
      sessionId,
      deliveryKind: "desktop-continuous",
      includeSnapshot: true,
    });

    return {
      sessionId,
      async send(prompt: string) {
        const result = (await request("session/send", { sessionId, content: prompt })) as
          | { accepted?: boolean }
          | undefined;
        if (result?.accepted !== true) {
          throw new Error(`oracle：session/send 未被受理：${JSON.stringify(result)}`);
        }
      },
      events: queue.iterable,
      frames,
      invalidLines,
      get stderr() {
        return stderrText;
      },
      get mockRequestPaths() {
        return mock.requestPaths;
      },
      async stop() {
        if (stopped) return;
        stopped = true;
        queue.close();
        // 还在等应答的请求不能悬着：进程要走，它们永远不会被应答了。
        for (const [id, waiter] of pending) {
          pending.delete(id);
          waiter.reject(new Error(`oracle：stop() 时请求 #${id} 仍未应答`));
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
        await mock.close();
      },
    };
  } catch (error) {
    queue.close();
    child?.kill();
    await mock.close();
    throw error;
  }
}

function delay(ms: number): Promise<void> {
  return new Promise((resolvePromise) => {
    setTimeout(resolvePromise, ms);
  });
}

async function withTimeout<T>(promise: Promise<T>, ms: number, what: string): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_resolve, rejectPromise) => {
        timer = setTimeout(() => rejectPromise(new Error(`${what} 超时（${ms}ms）`)), ms);
      }),
    ]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}

/** 供用例/录制脚本用：起一个临时工作目录并落盘种子文件。 */
export async function seedWorkspace(cwd: string, files: Record<string, string>): Promise<void> {
  await mkdir(cwd, { recursive: true });
  for (const [relativePath, content] of Object.entries(files)) {
    const target = join(cwd, relativePath);
    await mkdir(join(target, ".."), { recursive: true });
    await writeFile(target, content, "utf8");
  }
}
