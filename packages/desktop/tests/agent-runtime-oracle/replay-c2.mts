// C2 Task 8：Rust `agent-server` 的 C2 夹具回放 harness（compact / memory 对拍）。
//
// 与 C1 的 `replay-agent-server.mts` 同一纪律：**单一比较器**——白名单规则只有
// `normalize.mts`（事件面）与 `session-lifecycle-fixtures.mts` 的 `normalizeDeep`
// （DB 行 / 结构化投影；C2 夹具录制时用的就是它，见 `probe-c2-support.mts`），
// 比较只有 `compare.mts` 的 `compareEventStream` 与本文件的 `diffDeep`（纯结构
// 相等，不含任何规则）。C2 夹具的 `expect` 由 `probe-c2-*.mts` 从 oracle 录制，
// 本 harness **不修改夹具**；比较面是契约
// `docs/specs/agent-c2-contract.md` 登记的可比子集（各 driver 内逐条注明）。
//
// 与 C1 回放的差异：
// - **空数据根启动**（`freshStorage:true`）：夹具录制时 oracle 在空隔离根上自建库，
//   T7 起 Rust 也在空根上自迁移（fresh-install），预建 C1 基线库反而会写入伪造账本行；
// - **事件登记缺省过滤**：`session.updated` / `streamRecovery.updated` /
//   `session.titleUpdated` 两侧同删（C1 登记缺省类型；C2 compact/memory 的 Rust
//   事件面同口径，契约 §1.9「事件面」/§3.5）；
// - **DB 行投影**：`readC2Db`（`probe-c2-support.mts`）在两侧跑同一实现——夹具侧
//   已是录制时的输出，Rust 侧用回放 context 跑同一函数后做结构比较；
// - scripted provider 夹具：C2 夹具的剧本在 `input.script`，临时写出
//   `{script:…}` 形状供 `MODE_AGENT_SERVER_SCRIPTED_PROVIDER` 消费（不复制夹具）。
//
// 未观察/不可比子集（登记在 `tauri-agent-runtime.md` §C2，不在本 harness 硬凑）：
// scripted provider 下没有 mock HTTP 请求面（`isolation.mockRequestPaths` /
// `compactModelRequest` 只在 memory 对拍经本地 mock provider 观察，见
// `replay-c2-memory.mts`）；`compact-while-turn-running` 需要钉住「turn 正在执行」
// 的窗口，TS 侧无法确定性地构造（登记为未覆盖）。
//
// 单跑入口：`pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/replay-c2.mts [--case <name>]`
// 套件入口：`agent-server-c2-replay.test.mts`（`pnpm test:agent-runtime-oracle` 自动纳入）。
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { compareEventStream } from "./compare.mjs";
import { seedWorkspace } from "./driver.mjs";
import { startMockProvider, writeOracleProviderConfig, type MockStep } from "./mock-provider.mjs";
import { REPO_ROOT } from "./agent-server-env.mjs";
import { normalizeDeep, type JsonObject, type PathContext } from "./session-lifecycle-fixtures.mjs";
import { startAgentServer, type AgentServerEvent, type AgentServerSession } from "./agent-server-session.mjs";

export const C2_FIXTURE_DIR = fileURLToPath(new URL("./fixtures/c2/", import.meta.url));

/** compact/memory 的 Rust 可产出事件面：终态事件（契约 §1.9）。 */
const TERMINAL_EVENT_TYPES = new Set(["turn.completed", "turn.failed"]);
const TURN_TIMEOUT_MS = 90_000;

/**
 * C2 登记缺省过滤（契约 §1.9「事件面」/ §3.5；与 C1 的 `C1_ABSENT_EVENT_TYPES`
 * 同一张登记表——Rust 不产出这三类，两侧同删，除此之外不再有任何过滤）。
 */
export const C2_ABSENT_EVENT_TYPES: readonly string[] = [
  "session.updated",
  "streamRecovery.updated",
  "session.titleUpdated",
];

export type C2Fixture = {
  case: string;
  note: string;
  input: JsonObject;
  expect: JsonObject;
};

/** 差异形状沿用 `compare.mts` 的 Diff（layer 放宽为 C2 的比较层名）。 */
export type C2Diff = {
  layer: "events" | "db" | "response" | "rows";
  path: string;
  message: string;
  actual: unknown;
  expected: unknown;
};

export type C2ReplayResult = {
  case: string;
  diffs: C2Diff[];
  /** 时序相关结果的观察注记（如 same-tick 的 outcome；不算差异）。 */
  notes: string[];
  stderr: string;
};

/** 读一条 C2 夹具（`fixtures/c2/<case>.json`；只读，绝不回写）。 */
export async function loadC2Fixture(caseName: string): Promise<C2Fixture> {
  const module = await import(`${pathToFileURL(join(C2_FIXTURE_DIR, `${caseName}.json`)).href}`);
  return module.default as C2Fixture;
}

// ---------------------------------------------------------------------------
// 纯结构比较（无规则）：夹具侧与实际侧都已是「同一投影函数的输出」，
// 这里只做逐路径相等；数组按序、对象按键集合。
// ---------------------------------------------------------------------------

export function diffDeep(
  layer: C2Diff["layer"],
  path: string,
  actual: unknown,
  expected: unknown,
): C2Diff[] {
  if (stableEqual(actual, expected)) return [];
  if (
    typeof actual === "object" &&
    typeof expected === "object" &&
    actual !== null &&
    expected !== null &&
    !Array.isArray(actual) === !Array.isArray(expected)
  ) {
    const diffs: C2Diff[] = [];
    const actualRecord = actual as Record<string, unknown>;
    const expectedRecord = expected as Record<string, unknown>;
    for (const key of Object.keys(expectedRecord)) {
      if (!(key in actualRecord)) {
        diffs.push({ layer, path: `${path}.${key}`, message: "实际缺少键", actual: undefined, expected: expectedRecord[key] });
      }
    }
    for (const key of Object.keys(actualRecord)) {
      if (!(key in expectedRecord)) {
        diffs.push({ layer, path: `${path}.${key}`, message: "实际多出键", actual: actualRecord[key], expected: undefined });
        continue;
      }
      diffs.push(...diffDeep(layer, `${path}.${key}`, actualRecord[key], expectedRecord[key]));
    }
    return diffs;
  }
  return [{ layer, path, message: "值不同", actual, expected }];
}

/** 稳定串相等（键序无关；与 `compare.mts` 内部同一口径）。 */
function stableEqual(actual: unknown, expected: unknown): boolean {
  return JSON.stringify(sortValue(actual)) === JSON.stringify(sortValue(expected));
}

function sortValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortValue);
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>)
        .sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0))
        .map(([key, inner]) => [key, sortValue(inner)]),
    );
  }
  return value;
}

// ---------------------------------------------------------------------------
// 回放公共执行半边：隔离根 + scripted 夹具 + 终态等待
// ---------------------------------------------------------------------------

export type C2ReplayContext = {
  session: AgentServerSession;
  context: PathContext;
  dataRoot: string;
  cwd: string;
  dbPath: string;
  /** 只算会话事件帧（`session/event` 映射），供边界切分与终态计数。 */
  frames(): AgentServerEvent[];
  terminalCount(): number;
  waitForTerminals(expected: number): Promise<void>;
  /** 事件帧 → 登记缺省同删 → {type,payload} 规范化（比较层输入）。 */
  normalizedEvents(from: number): NormalizedEvent[];
  /** 夹具事件（录制时的完整 params）→ 同删 → {type,payload}（`normalizeDeep` 已在录制时应用）。 */
  normalizedFixtureEvents(events: JsonObject[]): NormalizedEvent[];
};

export type C2StartOptions = {
  fixture: C2Fixture;
  /** 覆盖偏好应答（memory 对拍要 memoryEnabled:true）；缺省 = spec §1.3 默认表。 */
  runtimePreferences?: Record<string, unknown>;
  /**
   * 本地 mock provider 剧本（`mock-provider.mts`）。提供时走**真实 hub 装配**
   * （`MODE_PERSONAL_PROVIDER_CONFIG_FILE` 指向隔离根内的 mock 配置），与 oracle
   * 录制条件同形状：模型能力（contextWindow 等）来自 provider 配置，请求体可观测，
   * 剧本语义同为「请求下标 = assistant 条数」。缺省走 `MODE_AGENT_SERVER_SCRIPTED_PROVIDER`
   * 的 scripted 装配（C1 三层回放同款；无模型能力事实，夹具快照面不可比）。
   */
  mockScript?: MockStep[];
  /** 复用已有隔离根/工作目录（memory 的「同一数据根重启」）；缺省 mkdtemp。 */
  dataRoot?: string;
  cwd?: string;
};

export type C2Run = C2ReplayContext & {
  stop(): Promise<void>;
  /** mock provider 收到的原始请求体（仅 mockScript 路径；scripted 装配下恒空）。 */
  readonly rawBodies: JsonObject[];
  /** mock provider 收到的请求路径（isolation.mockRequestPaths 的实际侧）。 */
  readonly mockRequestPaths: string[];
};

/**
 * 起一个 C2 回放运行：空隔离根（fresh-install）+ 事件收集。
 * `input.script` 在 scripted 路径下写临时夹具文件（scripted.rs 只认顶层 `script`）。
 */
export async function startC2Replay(
  fixture: C2Fixture,
  options: Pick<C2StartOptions, "runtimePreferences" | "mockScript" | "dataRoot" | "cwd"> = {},
): Promise<C2Run> {
  const prefix = `mode-c2-${fixture.case}-`;
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
    });
    closeMock = () => mock.close();
    providerConfigPath = join(dataRoot, "c2-mock-provider.json");
    await writeOracleProviderConfig(providerConfigPath, mock.baseUrl);
    builtinConfigPath = join(REPO_ROOT, "config", "provider", "mode-builtin.json");
  }
  const session = await startAgentServer({
    dataRoot,
    cwd,
    ...(providerConfigPath !== undefined
      ? { providerConfigPath, builtinConfigPath }
      : { fixturePath: await writeScriptedFixture(dataRoot, fixture) }),
    freshStorage: true,
    runtimePreferences: options.runtimePreferences,
  });
  const frames = (): AgentServerEvent[] => session.frames;
  const terminalCount = (): number =>
    session.frames.filter((event) => TERMINAL_EVENT_TYPES.has(event.type)).length;
  const run: C2Run = {
    session,
    context,
    dataRoot,
    cwd,
    dbPath: join(dataRoot, ".mode", "cli", "db", "db.sqlite"),
    frames,
    terminalCount,
    rawBodies,
    mockRequestPaths,
    async waitForTerminals(expected: number) {
      const deadline = Date.now() + TURN_TIMEOUT_MS;
      while (terminalCount() < expected) {
        if (Date.now() > deadline) {
          throw new Error(
            `replay-c2：${fixture.case} 在 ${TURN_TIMEOUT_MS}ms 内没有等到第 ${expected} 个终态。` +
              `stderr=${session.stderr}`,
          );
        }
        await delay(25);
      }
      // 与 C1/探针同一沉降窗：终态后的收尾帧落定。
      await delay(500);
    },
    normalizedEvents(from: number) {
      return session.frames
        .slice(from)
        .filter((event) => !C2_ABSENT_EVENT_TYPES.includes(event.type))
        .map((event) => ({
          type: event.type,
          // C2 夹具事件录制时过的是 `normalizeDeep`（`recordFrames`），实际侧
          // 必须用同一份投影——`normalizeValue` 是 C1 三层的事件投影，规则面不同。
          payload: normalizeDeep(event.payload ?? null, context),
        }));
    },
    normalizedFixtureEvents(events: JsonObject[]) {
      return events
        .filter((event) => !C2_ABSENT_EVENT_TYPES.includes(String(event.type)))
        .map((event) => ({
          type: String(event.type),
          // 夹具 payload 已经是录制时 `normalizeDeep` 的输出（`recordFrames`），
          // 不再二次规范化：session-lifecycle 的 `normalizeString` 对空前缀不设防
          //（空 context 会把记号插进每个字符间），而空 context 也带不进任何新规则。
          payload: event.payload ?? null,
        }));
    },
    stop() {
      return session.stop();
    },
  };
  (run as C2Run & { __closeMock?: () => Promise<void> }).__closeMock = closeMock;
  return run;
}

/** scripted 路径的临时夹具（C2 夹具的剧本在 `input.script`；scripted.rs 认顶层键）。 */
async function writeScriptedFixture(dataRoot: string, fixture: C2Fixture): Promise<string> {
  const scriptFile = join(dataRoot, "c2-scripted-fixture.json");
  await writeFile(scriptFile, JSON.stringify({ script: fixture.input.script ?? [] }), "utf8");
  return scriptFile;
}

/** 停止回放（含 mock）并删除隔离目录（所有 driver 的 finally 必须调用）。 */
export async function stopC2Replay(run: C2Run): Promise<void> {
  const closeMock = (run as C2Run & { __closeMock?: () => Promise<void> }).__closeMock;
  await run.stop();
  if (closeMock) await closeMock();
  await rm(run.dataRoot, { recursive: true, force: true });
  await rm(run.cwd, { recursive: true, force: true });
}

/** 发一轮 prompt 并等终态（与 C1 回放同一基线口径：基线在 send 前捕获）。 */
export async function sendTurnAndWait(run: C2Run, prompt: string): Promise<void> {
  const baseline = run.terminalCount();
  await run.session.send(prompt);
  await run.waitForTerminals(baseline + 1);
  const terminal = run
    .frames()
    .filter((event) => TERMINAL_EVENT_TYPES.has(event.type))
    .at(-1);
  if (terminal?.type === "turn.failed") {
    throw new Error(`replay-c2：turn.failed：${JSON.stringify(terminal.payload)}`);
  }
}

/** 读当前 `session/read` 的 `runtime.stateRevision`（compact 的 expectedRevision 输入）。 */
export async function readStateRevision(run: C2Run): Promise<number> {
  const read = (await run.session.request("session/read", {
    sessionId: run.session.sessionId,
  })) as { runtime?: { stateRevision?: number } } | undefined;
  const revision = read?.runtime?.stateRevision;
  if (typeof revision !== "number") {
    throw new Error(`replay-c2：session/read 没有 stateRevision：${JSON.stringify(read)}`);
  }
  return revision;
}

/** 用回放 context 跑 `readC2Db`（DB 行比较的实际侧；夹具侧是录制时的同一函数输出）。 */
export async function readReplayDb(run: C2Run): Promise<JsonObject> {
  const { readC2Db } = await import("./probe-c2-support.mjs");
  return readC2Db(run.dbPath, run.context) as JsonObject;
}

function delay(ms: number): Promise<void> {
  return new Promise((resolvePromise) => {
    setTimeout(resolvePromise, ms);
  });
}

// ---------------------------------------------------------------------------
// 事件层比较：`compare.mts` 的同一份 `compareEventStream`
// ---------------------------------------------------------------------------

export function diffEventStream(
  actual: NormalizedEvent[],
  expected: NormalizedEvent[],
): C2Diff[] {
  return compareEventStream(actual, expected).map((diff) => ({ ...diff, layer: "events" as const }));
}

// ---------------------------------------------------------------------------
// DB 行比较的登记折叠（两侧同折；每条都对应契约/差异登记的一行，见
// `tauri-agent-runtime.md` §C2 差异登记）：
//
// 1. `preCompactTokenCount` / `truePostCompactTokenCount`：本地估算值，oracle 的
//    公式未观察（契约 §1.9 明确登记「本地定义」），Rust 按自己的口径估算——
//    绝对值不比，两侧同删。
// 2. 运行时生成的 **message-id 形状**：契约 §1.9「生成 id 只对齐非空 id 形状」。
//    oracle 的 `msg_<base36>_<uuid>` 在录制折叠后是 `msg_…_<id>`，Rust 的
//    `msg_<millis>_<seq>` 不含 uuid、共享折叠规则吃不到——对下列键做两侧同折：
//    `tail_start_id` / `lastSummarizedMessageId` / `summaryMessageIds`（列表）。
// 3. prompt turn 的行形状（timeline_event 会话行、user/assistant 行字段、
//    per-turn `model_usage`、`session_entry` 第三行、title 三列）：C1 时代的
//    DB 层差异，此前任何门禁都没有比过 DB——T8 新发现，登记为「DB 层未对齐面」，
//    比较面只取 C2 拥有的行（compact/memory 增量行），见各 driver。
// ---------------------------------------------------------------------------

/** 值形状是运行时生成的 message-id（oracle 折叠形或 Rust 原生形都算）。 */
function isMessageIdShape(value: unknown): value is string {
  return (
    typeof value === "string" &&
    (value === "<message-id>" ||
      /^msg_[a-z0-9]+_<(?:id|message-id)>$/iu.test(value) ||
      /^msg_\d+_\d+$/iu.test(value) ||
      /^msg_[a-z0-9]+_[0-9a-f-]{36}(?:_message)?$/iu.test(value))
  );
}

/**
 * 登记折叠 2：运行时生成 id 的**形状键**两侧同折（契约 §1.9「生成 id 只对齐
 * 非空 id 形状」）——`tail_start_id` / `lastSummarizedMessageId` /
 * `summaryMessageIds`（列表）→ `<message-id>`；`currentTurnId` → `<turn-id>`。
 */
export function foldRegisteredMessageIds(value: unknown, key?: string): unknown {
  if (
    (key === "tail_start_id" || key === "lastSummarizedMessageId") &&
    isMessageIdShape(value)
  ) {
    return "<message-id>";
  }
  if (key === "summaryMessageIds" && Array.isArray(value) && value.every((item) => isMessageIdShape(item))) {
    return ["<message-id>"];
  }
  if (key === "currentTurnId" && (value === "<turn-id>" || /^turn_\d+$/u.test(String(value)) || /^turn_[0-9a-f-]{36}$/iu.test(String(value)))) {
    return "<turn-id>";
  }
  if (Array.isArray(value)) return value.map((item) => foldRegisteredMessageIds(item, key));
  if (typeof value !== "object" || value === null) return value;
  const out: Record<string, unknown> = {};
  for (const [innerKey, inner] of Object.entries(value as Record<string, unknown>)) {
    out[innerKey] = foldRegisteredMessageIds(inner, innerKey);
  }
  return out;
}

/** 登记折叠 1：本地估算 token 数（绝对值不比）。数组原样保留（逐项折叠）。 */
export function foldLocalTokenEstimates(row: unknown): unknown {
  if (Array.isArray(row)) return row.map((item) => foldLocalTokenEstimates(item));
  if (typeof row !== "object" || row === null) return row;
  const record = { ...(row as Record<string, unknown>) };
  delete record.preCompactTokenCount;
  delete record.truePostCompactTokenCount;
  const out: Record<string, unknown> = {};
  for (const [key, inner] of Object.entries(record)) {
    out[key] =
      inner !== null && typeof inner === "object" ? foldLocalTokenEstimates(inner) : inner;
  }
  return out;
}

/** DB 行比较入口：先过登记折叠（两侧同一实现），再做结构 diff。 */
export function diffDbRows(path: string, actual: unknown, expected: unknown): C2Diff[] {
  return diffDeep("db", path, foldRegisteredMessageIds(foldLocalTokenEstimates(actual)), foldRegisteredMessageIds(foldLocalTokenEstimates(expected)));
}

// ---------------------------------------------------------------------------
// case 注册表 + CLI 入口（与 replay-agent-server.mts 同形状）
// ---------------------------------------------------------------------------

export type C2CaseDriver = (fixture: C2Fixture) => Promise<C2ReplayResult>;

/** C2 回放注册表：compact 六条 + memory 三条（driver 在 `replay-c2-*.mts`）。 */
export const C2_REPLAY_CASES: Record<string, C2CaseDriver> = {};

// driver 模块在 CLI 入口静态引入（注册表在入口处填充；测试套件自填，见
// `agent-server-c2-replay.test.mts`）。ESM 环在这里是安全的：driver 模块只在
// 函数体内使用本模块的绑定，不在模块求值期取用。
import { registerCompactCases } from "./replay-c2-compact.mjs";
import { registerMemoryCases } from "./replay-c2-memory.mjs";

const invoked = process.argv[1] ? fileURLToPath(pathToFileURL(process.argv[1]).href) : undefined;
const isMain =
  invoked !== undefined && invoked.toLowerCase() === fileURLToPath(import.meta.url).toLowerCase();
if (isMain) {
  registerCompactCases(C2_REPLAY_CASES);
  registerMemoryCases(C2_REPLAY_CASES);
  const caseIndex = process.argv.indexOf("--case");
  const only = caseIndex >= 0 ? process.argv[caseIndex + 1] : undefined;
  const names = Object.keys(C2_REPLAY_CASES).filter((name) => only === undefined || name === only);
  if (names.length === 0) {
    process.stderr.write(`没有匹配的 C2 case：${only ?? "(全部)"}\n`);
    process.exitCode = 1;
  } else {
    let green = 0;
    for (const name of names) {
      const fixture = await loadC2Fixture(name.split(":")[0] as string);
      const result = await C2_REPLAY_CASES[name]?.(fixture);
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
    process.stdout.write(`\n${green}/${names.length} 条 C2 夹具回放一致。\n`);
    if (green !== names.length) process.exitCode = 1;
  }
}
