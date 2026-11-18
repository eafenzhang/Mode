// C3 Task 5：Rust `agent-server` 的 C3 夹具回放 harness（goal / retry / background / subagents 对拍）。
//
// 与 C1（`replay-agent-server.mts`）/ C2（`replay-c2.mts`）同一纪律：**单一比较器**——
// 投影规则只有 `probe-c3-support.mts` 的 `normalizeC3Deep`（与录制时同一份函数，两侧同投）、
// 本文件的登记折叠（生成 id 形状，两侧同折）与登记缺省过滤（两侧同删），比较只有
// `compare.mts` 的 `compareEventStream` 与 `replay-c2.mts` 的 `diffDeep`（纯结构相等）。
// C3 夹具的 `expect` 由 `probe-c3-*.mts` 从 oracle 录制，本 harness **不修改夹具**；
// 比较面是契约 `docs/specs/agent-c3-contract.md` §1.11/§2.7/§3.5/§4.8 登记的可比子集
// （汇总登记见 `tauri-agent-runtime.md` §C3）。
//
// 驱动用与 oracle 录制同形状的**本地 mock provider**（模型能力事实来自 provider 配置）；
// goal verifier 的应答经 `auxiliaryResponse` 注入（与 probe 的 `mockAuxiliaryResponse`
// 同一注入点）。scripted 装配没有能力事实，夹具快照面不可比——与 C2 同理不用。
//
// 登记缺省过滤（`isC3ComparableEvent`，两侧同删；除此之外不再有任何过滤）：
// 1. 事件信封投影：只比 `{type, payload}`——`seq`/`eventId`/`timestamp`/`traceId`/
//    `turnId`/`sessionId`/`deliveryKind` 的绝对值是运行时起点事实（登记差异）。
// 2. `session.updated` 带 `messageCount` / `content` 键：C1 缺省事件族（请求边界/内容推送）。
// 3. `session.updated` `type:"model_request_started"`：C3 未产出（§2.7）。
// 4. `session.updated` `type:"model_request_failed"` 且 reason 非 cancelled：C3 只产出
//    取消收口事件（§2.7）。
// 5. `session.updated` `type:"model_request_completed"`：只在 retry/recovery 收口产出；
//    非 retry 用例两侧同删（§2.7）。
// 6. `session.updated` 带 `taskId`（任务状态）：C3 未产出（§3.5）。
// 7. `session.updated` 带 `agentType`（子代理生命周期）：C3 未产出（§4.3 归 C4）。
// 8. `session.titleUpdated`：goal 路径已实现（§1.1 副作用）——goal 用例保留；普通 prompt
//    用例两侧同删（C1 title 缺省）。
// 9. `streamRecovery.updated` 带 `kind` 键：C1 工具结果锚点形状（§2.1），Rust 只产出
//    recovery 流形状（§2.2）——锚点形状两侧同删。
// 10. goal-continuation `turn.started` 的 `input`：提示词正文 clean-room 自撰（§1.11），
//     两侧同删（其余 turn.started 的 input 逐字比）。
//
// 登记折叠（`foldC3GeneratedIds`，两侧同折；对夹具 token 幂等）：Rust 生成 id 是
// `<prefix>_<毫秒>_<计数>`，oracle 是 base36+uuid——按键/形状折到同一 token：
// `messageId`/`verificationId` → `<id>`；`anchorAssistantMessageId` → `<message-id>`；
// `anchorTurnId` → `<turn-id>`；形状 `msg_\d+_\d+` → `<message-id>`、`turn_\d+` →
// `<turn-id>`、`<前缀>_<数字>_<数字>` → `<id>`。重试事件未接线字段（§2.7）两侧同删：
// `baseURL`/`providerKind`/`transport`/`requestHeader(s)`/`responseHeader(s)`/
// `exceptionType`/`streamOutputCommitted`/timing 五键。
//
// DB 比较面（`diffC3DbRows` 登记）：只取 C3 拥有的行——`session_target`/`session_task_link`
// 全量、`session_entry` 的 target_completion_verification 行、`turn_usage`/`model_usage`
// （goal 用例）/`messageSummary`。登记折叠：`turn_usage` 的 `trace_id`/`first_model_start_at`/
// `first_token_at`（C1 时代 DB 层未对齐面，§C2.8 #24）、messageSummary 的 timeline_event
// 行（#22/#23 合成行）与 `sequence` 位置、session 行 `title`（#26）；retry/background 用例的
// `model_usage` 整表同删（§2.7：C1 普通 turn 不写，夹具的逐 attempt 行没有复刻）。
//
// 单跑入口：`pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/replay-c3.mts [--case <name>]`
// 套件入口：`agent-server-c3-replay.test.mts`（`pnpm test:agent-runtime-oracle` 自动纳入）。
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { seedWorkspace } from "./driver.mjs";
import { startMockProvider, writeOracleProviderConfig, type MockAuxiliaryResponse, type MockStep } from "./mock-provider.mjs";
import { REPO_ROOT } from "./agent-server-env.mjs";
import type { C2Diff } from "./replay-c2.mjs";
import { startAgentServer, type AgentServerEvent, type AgentServerSession } from "./agent-server-session.mjs";
import {
  createC3Aliases,
  normalizeC3Deep,
  readC3Db,
  type C3AliasState,
  type PathContext,
} from "./probe-c3-support.mjs";
import {
  diffC3Value,
  diffGoalDb,
  GOAL_EVENT_SCOPE,
  PLAIN_EVENT_SCOPE,
  projectC3Event,
  projectC3Events,
  RETRY_EVENT_SCOPE,
  type C3EventScope,
  type NormalizedC3Event,
} from "./replay-c3-compare.mjs";
import type { JsonObject } from "./session-lifecycle-fixtures.mjs";

export const C3_FIXTURE_DIR = fileURLToPath(new URL("./fixtures/c3/", import.meta.url));

const TERMINAL_EVENT_TYPES = new Set(["turn.completed", "turn.failed"]);
const TURN_TIMEOUT_MS = 120_000;

export type C3Fixture = {
  case: string;
  note: string;
  input: JsonObject;
  expect: JsonObject;
};

/** 差异形状沿用 C2 的 Diff（layer 语义相同）。 */
export type C3Diff = C2Diff;

// 比较面（登记过滤/折叠/diff 入口）在 `replay-c3-compare.mts`；这里显式再导出，
// driver 与测试套件的导入面保持单一入口。
export {
  diffC3Value,
  diffEventStream,
  diffGoalDb,
  foldMessageSummary,
  GOAL_EVENT_SCOPE,
  isC3ComparableEvent,
  foldC3GeneratedIds,
  PLAIN_EVENT_SCOPE,
  projectC3Event,
  RETRY_EVENT_SCOPE,
} from "./replay-c3-compare.mjs";
export type { C3EventScope, NormalizedC3Event } from "./replay-c3-compare.mjs";

export type C3ReplayResult = {
  case: string;
  diffs: C3Diff[];
  /** 时序相关结果的观察注记（不算差异）。 */
  notes: string[];
  stderr: string;
};

/** 读一条 C3 夹具（`fixtures/c3/<case>.json`；只读，绝不回写）。 */
export async function loadC3Fixture(caseName: string): Promise<C3Fixture> {
  const module = await import(`${pathToFileURL(join(C3_FIXTURE_DIR, `${caseName}.json`)).href}`);
  return module.default as C3Fixture;
}

// ---------------------------------------------------------------------------
// 登记缺省过滤 + 登记折叠（见文件头；两侧同一实现）
// ---------------------------------------------------------------------------

/** 用例级过滤开关：goal 用例保留 titleUpdated；retry 用例保留 model_request_completed。 */
// 回放公共执行半边：隔离根 + mock provider + 事件收集
// ---------------------------------------------------------------------------

export type C3Run = {
  session: AgentServerSession;
  context: PathContext;
  aliases: C3AliasState;
  dataRoot: string;
  cwd: string;
  dbPath: string;
  /** mock provider 收到的原始请求体（与 probe 的 `requests[].body` 同源）。 */
  readonly rawBodies: JsonObject[];
  /** mock provider 收到的请求路径（isolation.mockRequestPaths 的实际侧）。 */
  readonly mockRequestPaths: string[];
  frames(): AgentServerEvent[];
  terminalCount(): number;
  waitForTerminals(expected: number, timeoutMs?: number): Promise<void>;
  /** 从 `from` 下标起等待满足条件的事件（谓词作用在**原始**帧上）。 */
  waitForEvent(
    from: number,
    predicate: (event: { type: string; payload: unknown }) => boolean,
    what: string,
    timeoutMs?: number,
  ): Promise<{ type: string; payload: unknown }>;
  /** 实际事件 → 登记过滤/折叠（比较层输入）。 */
  normalizedActualEvents(from: number, scope: C3EventScope, foldContinuationInput?: boolean): NormalizedC3Event[];
  /** 夹具事件（录制时的完整信封）→ 同一过滤/折叠（比较层输入）。 */
  normalizedFixtureEvents(events: JsonObject[], scope: C3EventScope, foldContinuationInput?: boolean): NormalizedC3Event[];
  /** 读 C3 全表（每次调用读当前磁盘状态；与 probe 的 `readTables` 同一实现）。 */
  readTables(): JsonObject;
  /** `session/read` 的 C3 规范化投影。 */
  readSessionNormalized(): Promise<JsonObject>;
  stop(): Promise<void>;
};

export type C3StartOptions = {
  fixture: C3Fixture;
  mockScript?: MockStep[];
  /** 非流式/无工具请求（goal verifier）的应答注入；缺省回 `oracle-mock`（非法 JSON）。 */
  auxiliaryResponse?: (body: JsonObject) => MockAuxiliaryResponse | undefined;
  /** 复用已有隔离根/工作目录（冷重启的「同一数据根」）；缺省 mkdtemp。 */
  dataRoot?: string;
  cwd?: string;
};

export async function startC3Replay(options: C3StartOptions): Promise<C3Run> {
  const { fixture } = options;
  const prefix = `mode-c3-${fixture.case}-`;
  const dataRoot = options.dataRoot ?? (await mkdtemp(join(tmpdir(), prefix)));
  const cwd = options.cwd ?? (await mkdtemp(join(tmpdir(), `${prefix}ws-`)));
  const context: PathContext = { dataRoot, cwd, tmpDir: tmpdir() };
  const seed = (fixture.input.seed ?? {}) as Record<string, string>;
  if (Object.keys(seed).length > 0) await seedWorkspace(cwd, seed);
  const rawBodies: JsonObject[] = [];
  const mockRequestPaths: string[] = [];
  let closeMock: (() => Promise<void>) | undefined;
  let providerConfigPath: string | undefined;
  let builtinConfigPath: string | undefined;
  if (options.mockScript !== undefined) {
    const mock = await startMockProvider(options.mockScript, {
      onRequestBody: (path, body) => {
        mockRequestPaths.push(path);
        rawBodies.push(body as JsonObject);
      },
      auxiliaryResponse: options.auxiliaryResponse,
    });
    closeMock = () => mock.close();
    providerConfigPath = join(dataRoot, "c3-mock-provider.json");
    await writeOracleProviderConfig(providerConfigPath, mock.baseUrl);
    builtinConfigPath = join(REPO_ROOT, "config", "provider", "mode-builtin.json");
  }
  const session = await startAgentServer({
    dataRoot,
    cwd,
    ...(providerConfigPath !== undefined
      ? { providerConfigPath, builtinConfigPath }
      : { fixturePath: join(dataRoot, "c3-no-scripted-provider.json") }),
    freshStorage: true,
  });
  const aliases = createC3Aliases();
  const frames = (): AgentServerEvent[] => session.frames;
  const terminalCount = (): number =>
    session.frames.filter((event) => TERMINAL_EVENT_TYPES.has(event.type)).length;
  const run: C3Run = {
    session,
    context,
    aliases,
    dataRoot,
    cwd,
    dbPath: join(dataRoot, ".mode", "cli", "db", "db.sqlite"),
    rawBodies,
    mockRequestPaths,
    frames,
    terminalCount,
    async waitForTerminals(expected: number, timeoutMs = TURN_TIMEOUT_MS) {
      const deadline = Date.now() + timeoutMs;
      while (terminalCount() < expected) {
        if (Date.now() > deadline) {
          throw new Error(
            `replay-c3：${fixture.case} 在 ${timeoutMs}ms 内没有等到第 ${expected} 个终态。` +
              `stderr=${session.stderr}`,
          );
        }
        await delay(25);
      }
      await delay(500);
    },
    async waitForEvent(from, predicate, what, timeoutMs = 90_000) {
      const deadline = Date.now() + timeoutMs;
      while (true) {
        const found = session.frames
          .slice(from)
          .find((event) => predicate({ type: event.type, payload: event.payload }));
        if (found !== undefined) return { type: found.type, payload: found.payload };
        if (Date.now() > deadline) {
          throw new Error(`replay-c3：${fixture.case} 在 ${timeoutMs}ms 内没有观察到 ${what}`);
        }
        await delay(20);
      }
    },
    normalizedActualEvents(from: number, scope: C3EventScope, foldContinuationInput = true) {
      // 实际侧先过与录制同一份 `normalizeC3Deep`（路径/id/时间/别名投影），再走
      // 登记过滤与折叠；夹具侧已是录制时输出，不再二次规范化。
      return projectC3Events(
        session.frames
          .slice(from)
          .map((event) => ({
            type: event.type,
            payload: normalizeC3Deep(event.payload ?? null, context, aliases),
          })),
        scope,
        foldContinuationInput,
      );
    },
    normalizedFixtureEvents(events: JsonObject[], scope: C3EventScope, foldContinuationInput = true) {
      return projectC3Events(
        events.map((event) => ({ type: String(event.type), payload: event.payload ?? null })),
        scope,
        foldContinuationInput,
      );
    },
    readTables() {
      return readC3Db(run.dbPath, context, aliases);
    },
    async readSessionNormalized() {
      const result = await session.request("session/read", { sessionId: session.sessionId });
      return normalizeC3Deep(result, context, aliases) as JsonObject;
    },
    stop() {
      return session.stop();
    },
  };
  (run as C3Run & { __closeMock?: () => Promise<void> }).__closeMock = closeMock;
  return run;
}

/** 停止回放（含 mock）并删除隔离目录（所有 driver 的 finally 必须调用）。 */
export async function stopC3Replay(run: C3Run): Promise<void> {
  const closeMock = (run as C3Run & { __closeMock?: () => Promise<void> }).__closeMock;
  await run.stop();
  if (closeMock) await closeMock();
  // Windows 上后台 Bash 子进程/日志句柄可能在进程退出后短暂锁住目录：
  // 带退避重试，最终失败降级为警告（不影响比较结论，也不掩盖回放差异）。
  await rmWithRetry(run.dataRoot);
  await rmWithRetry(run.cwd);
}

async function rmWithRetry(path: string): Promise<void> {
  for (const attempt of [0, 200, 500, 1000, 2000]) {
    try {
      await rm(path, { recursive: true, force: true });
      return;
    } catch (error) {
      if (attempt === 2000) {
        process.stderr.write(`replay-c3：临时目录清理失败（保留 ${path}）：${String(error)}\n`);
        return;
      }
      await delay(attempt);
    }
  }
}

function delay(ms: number): Promise<void> {
  return new Promise((resolvePromise) => {
    setTimeout(resolvePromise, ms);
  });
}

// ---------------------------------------------------------------------------
// 比较入口：事件层（compare.mts 单一比较器）+ DB 层（登记折叠 + diffDeep）
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// case 注册表 + CLI 入口（与 replay-c2.mts 同形状）
// ---------------------------------------------------------------------------

export type C3CaseDriver = (fixture: C3Fixture) => Promise<C3ReplayResult>;

/** C3 回放注册表：driver 在 `replay-c3-*.mts` 注册。 */
export const C3_REPLAY_CASES: Record<string, C3CaseDriver> = {};

// driver 模块在 CLI 入口静态引入（注册表在入口处填充；测试套件自填，见
// `agent-server-c3-replay.test.mts`）。ESM 环在这里是安全的：driver 模块只在
// 函数体内使用本模块的绑定，不在模块求值期取用（与 replay-c2.mts 同款）。
import { registerC3GoalCases } from "./replay-c3-goal.mjs";
import { registerC3BackgroundCases } from "./replay-c3-background.mjs";
import { registerC3GoalConflictCases } from "./replay-c3-goal-conflict.mjs";
import { registerC3MixedCases } from "./replay-c3-mixed.mjs";

const invoked = process.argv[1] ? fileURLToPath(pathToFileURL(process.argv[1]).href) : undefined;
const isMain =
  invoked !== undefined && invoked.toLowerCase() === fileURLToPath(import.meta.url).toLowerCase();
if (isMain) {
  registerC3GoalCases(C3_REPLAY_CASES);
  registerC3BackgroundCases(C3_REPLAY_CASES);
  registerC3GoalConflictCases(C3_REPLAY_CASES);
  registerC3MixedCases(C3_REPLAY_CASES);
  const caseIndex = process.argv.indexOf("--case");
  const only = caseIndex >= 0 ? process.argv[caseIndex + 1] : undefined;
  const names = Object.keys(C3_REPLAY_CASES).filter((name) => only === undefined || name === only);
  if (names.length === 0) {
    process.stderr.write(`没有匹配的 C3 case：${only ?? "(全部)"}\n`);
    process.exitCode = 1;
  } else {
    let green = 0;
    for (const name of names) {
      const fixture = await loadC3Fixture(name.split(":")[0] as string);
      const result = await C3_REPLAY_CASES[name]?.(fixture);
      if (!result) {
        process.stderr.write(`没有 case driver：${name}\n`);
        process.exitCode = 1;
        continue;
      }
      for (const note of result.notes) process.stdout.write(`  NOTE ${name}: ${note}\n`);
      if (result.diffs.length === 0) {
        green += 1;
        process.stdout.write(`GREEN ${name}\n`);
      } else {
        process.stdout.write(
          `RED ${name}（${result.diffs.length} 处差异）\n` +
            result.diffs
              .slice(0, 20)
              .map(
                (diff) =>
                  `  - [${diff.layer}] ${diff.path}: ${diff.message}\n    实际：${JSON.stringify(diff.actual)}\n    期望：${JSON.stringify(diff.expected)}`,
              )
              .join("\n") +
            "\n",
        );
      }
    }
    process.stdout.write(`\n${green}/${names.length} 条 C3 夹具回放一致。\n`);
    if (green !== names.length) process.exitCode = 1;
  }
}
