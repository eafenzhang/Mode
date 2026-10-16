// M2 Task 1：协议夹具的录制入口（spec §1.2 的线上形状，ledger Ruling 13）。
//
// 用法：pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/record-protocol-fixtures.mts
//
// 纪律（与 Task 0 的 record.mts 同口径，ledger Ruling 6）：
//   夹具**只能录不能写**。这里跑两遍旧运行时（读成功 / 读不存在），把 stdout / stdin 上
//   **整帧原样**挑出来落盘——不裁剪、不手改、不合成。挑选是按 method/type/kind 谓词
//   「找不到就抛错」（fail-closed），所以夹具内容一定来自真实运行。
//
// 为什么不裁剪：Brief 允许「再裁到被测字段」，但本任务选的帧本来就短（最大 ~800B），
//   原样保留反而更容易核对（把任一帧贴回一次新捕获的 stdout 即可逐字比对）；
//   裁剪是一次人工编辑，会削弱「这是真实帧」这个结论。
//
// 产物：packages/shared/test/fixtures/agent-protocol/*.json —— **两端共读同一个目录**：
//   - TS 侧 packages/desktop/tests/agent-runtime-oracle/protocol-fixtures.test.mts
//     用 `mode-protocol`（v1）的真实 zod schema 校验；
//   - Rust 侧 crates/agent-protocol/tests/fixtures.rs 用 serde 镜像类型校验。
//
// 录完再跑一次 `pnpm exec oxfmt packages/shared/test/fixtures/agent-protocol/`：
// 录制器按 `JSON.stringify` 落盘，格式由仓库格式化器统一（与 Task 0 的 record.mts 同口径）。
import { mkdtemp, mkdir, readdir, rm, unlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { startOracleRuntime, seedWorkspace } from "./driver.mjs";
import type { MockStep } from "./mock-provider.mjs";

/** 一次 oracle 运行捕获到的双向原始帧（stdout = 运行时→host，stdin = host→运行时）。 */
type WireTrace = {
  stdout: Record<string, unknown>[];
  stdin: Record<string, unknown>[];
};

/** 夹具的外壳：`frame` 是线上整帧，其余是我们的记录元数据（不属于线上形状）。 */
type ProtocolFixture = {
  case: string;
  direction: "runtime->host" | "host->runtime";
  /**
   * 这句帧应答的是哪条 host 请求（仅 host→runtime 的应答帧有）。
   * 由录制时的 id 关联**从捕获推出来**，不是手写：JSON-RPC 靠 id 关联，
   * 而 id 不进帧内容，所以夹具必须自己把这条关系记下来。
   */
  respondsTo?: string;
  /** 这一帧为什么进夹具（人读用）。 */
  note: string;
  frame: Record<string, unknown>;
};

const FIXTURE_DIR = fileURLToPath(
  new URL("../../../shared/test/fixtures/agent-protocol/", import.meta.url),
);

const TERMINAL_EVENT_TYPES = new Set(["turn.completed", "turn.failed"]);

/** 两遍 oracle：一次读成功（有 result/batch），一次读不存在（有 error）。 */
const RUNS: readonly { script: MockStep[]; seed: Record<string, string>; prompt: string }[] = [
  {
    script: [
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/hello.txt" } }] },
      { text: "hello.txt 里写的是 hello。" },
    ],
    seed: { "hello.txt": "hello\n" },
    prompt: "读一下 hello.txt，回一句话说明它写了什么。",
  },
  {
    script: [
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/missing.txt" } }] },
      { text: "missing.txt 不存在，读取失败。" },
    ],
    seed: { "hello.txt": "hello\n" },
    prompt: "读一下 missing.txt；读不到就回一句话说明失败原因。",
  },
];

async function captureOnce(run: (typeof RUNS)[number]): Promise<WireTrace> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-protocol-fixture-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-protocol-fixture-ws-"));
  const trace: WireTrace = { stdout: [], stdin: [] };
  try {
    await seedWorkspace(cwd, run.seed);
    const session = await startOracleRuntime({
      dataRoot,
      cwd,
      script: run.script,
      onRawFrame: (frame) => trace.stdout.push(frame),
      onRawWrite: (frame) => trace.stdin.push(frame),
    });
    try {
      await session.send(run.prompt);
      const deadline = Date.now() + 90_000;
      for await (const event of session.events) {
        if (TERMINAL_EVENT_TYPES.has(event.type)) break;
        if (Date.now() > deadline) throw new Error("oracle 会话 90s 内没有终态事件");
      }
      // 终态之后还有收尾帧；给一小段沉降时间再收手。
      await new Promise((resolve) => setTimeout(resolve, 800));
      // 让运行时发出**真实的错误应答帧**（错误形状只在这里出现）。
      // 用一条 M2 真会发的命令 + 不存在的 sessionId，命中协议里已声明的 -32004。
      await session
        .request("session/send", { sessionId: "sess_does_not_exist", content: "x" })
        .catch(() => undefined);
    } finally {
      await session.stop();
    }
    return trace;
  } finally {
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
}

/** 取第一帧匹配的；找不到直接抛错（绝不合成一帧来补齐）。 */
function pick(
  trace: WireTrace,
  side: keyof WireTrace,
  what: string,
  match: (frame: Record<string, unknown>) => boolean,
): Record<string, unknown> {
  const frame = trace[side].find(match);
  if (!frame) throw new Error(`捕获里没有这一帧：${what}（${side}）`);
  return frame;
}

const isNotification = (method: string) => (frame: Record<string, unknown>) =>
  frame.method === method && frame.id === undefined;

const isEvent = (type: string) => (frame: Record<string, unknown>) =>
  isNotification("session/event")(frame) &&
  (frame.params as Record<string, unknown> | undefined)?.type === type;

const isToolUpdated = (kind: string) => (frame: Record<string, unknown>) => {
  if (!isEvent("tool.updated")(frame)) return false;
  return (frame.params as { payload?: { kind?: string } }).payload?.kind === kind;
};

const isReverseRequest = (method: string) => (frame: Record<string, unknown>) =>
  frame.method === method && frame.id !== undefined;

const isErrorFrame = (frame: Record<string, unknown>) =>
  frame.error !== undefined && frame.id !== undefined;

/** 这一帧是不是对某条指定方法的 host 请求的应答（靠 id 关联）。 */
const isResponseToMethod =
  (trace: WireTrace, method: string) => (frame: Record<string, unknown>) => {
    if (frame.result === undefined || frame.id === undefined) return false;
    return requestMethodById(trace, frame.id) === method;
  };

/**
 * 从一个 id 反查它属于哪条请求的方法名。
 *
 * 两个方向都要查：host 请求（stdin，如 session/send）的应答在 stdout；而运行时的反向请求
 * （stdout，如 session/requestRuntimePreferences，id 形如 "server-1"）的应答在 stdin。
 * 两侧共用同一个 id 空间，所以先查 stdin 再查 stdout 就是完整覆盖。
 */
function requestMethodById(trace: WireTrace, id: unknown): string | undefined {
  const owner = [...trace.stdin, ...trace.stdout].find(
    (frame) => frame.method !== undefined && frame.id === id,
  );
  return typeof owner?.method === "string" ? owner.method : undefined;
}

const success = await captureOnce(RUNS[0]);
const failure = await captureOnce(RUNS[1]);

/** 应答帧夹具：`respondsTo` 由 id 关联从捕获推出来，不手写。 */
function responseFixture(
  trace: WireTrace,
  method: string,
  fixtureCase: string,
  note: string,
): ProtocolFixture {
  const frame = pick(trace, "stdout", `${method} 的应答`, isResponseToMethod(trace, method));
  const respondsTo = requestMethodById(trace, frame.id);
  if (respondsTo === undefined) {
    throw new Error(`${fixtureCase}：应答帧没有解析出 respondsTo，夹具不自描述`);
  }
  return { case: fixtureCase, direction: "runtime->host", respondsTo, note, frame };
}

const fixtures: ProtocolFixture[] = [
  {
    case: "reverse-request-runtime-preferences",
    direction: "runtime->host",
    note: "运行时向 host 要运行时偏好（反向请求）；不应答会让 session/create 超时。注意 id 是字符串 server-N，不是数字。",
    frame: pick(
      success,
      "stdout",
      "反向请求 session/requestRuntimePreferences",
      isReverseRequest("session/requestRuntimePreferences"),
    ),
  },
  responseFixture(
    success,
    "session/send",
    "response-session-send",
    "session/send 的应答（受理确认）。id 与 host 发出的那条请求相同。",
  ),
  {
    case: "error-response-session-unavailable",
    direction: "runtime->host",
    note: "运行时发出的错误应答帧；code -32004 就是协议里声明的 sessionUnavailable。",
    frame: pick(success, "stdout", "错误应答帧", isErrorFrame),
  },
  {
    case: "session-event-title-updated",
    direction: "runtime->host",
    note: "首个会话事件（本地生成的首句标题，不依赖模型）。",
    frame: pick(success, "stdout", "session.titleUpdated", isEvent("session.titleUpdated")),
  },
  {
    case: "session-event-turn-started",
    direction: "runtime->host",
    note: "turn 起点。载荷里的 executionStartedAt 是线上字段，而 v1 共享 schema 尚未声明它（见 TS 侧的已知滞后表）。",
    frame: pick(success, "stdout", "turn.started", isEvent("turn.started")),
  },
  {
    case: "session-event-model-streaming-tool-call",
    direction: "runtime->host",
    note: "第一层（工具调用序列）入参的唯一来源：kind=tool_call 且带 input。",
    frame: pick(
      success,
      "stdout",
      "model.streaming/tool_call",
      (frame) =>
        isEvent("model.streaming")(frame) &&
        (frame.params as { payload?: { kind?: string } }).payload?.kind === "tool_call",
    ),
  },
  {
    case: "session-event-tool-updated-scheduled",
    direction: "runtime->host",
    note: "调度事件：故意不带 input（inputOmitted + inputRef），带并行分组与执行顺序容器。",
    frame: pick(success, "stdout", "tool.updated/scheduled", isToolUpdated("scheduled")),
  },
  {
    case: "session-event-tool-updated-started",
    direction: "runtime->host",
    note: "开始事件。载荷里的 readOnly / sideEffectScope 是线上字段，而 v1 共享 schema 尚未声明它们（见 TS 侧的已知滞后表）。",
    frame: pick(success, "stdout", "tool.updated/started", isToolUpdated("started")),
  },
  {
    case: "session-event-tool-updated-result",
    direction: "runtime->host",
    note: "成功结果：result.success / result.content 是工具输出层的取值处。",
    frame: pick(success, "stdout", "tool.updated/result", isToolUpdated("result")),
  },
  {
    case: "session-event-tool-updated-error",
    direction: "runtime->host",
    note: "失败结果（读不存在的文件）：载荷走 error 分支，没有 result。",
    frame: pick(failure, "stdout", "tool.updated/error", isToolUpdated("error")),
  },
  {
    case: "session-event-tool-updated-batch",
    direction: "runtime->host",
    note: "并行批次完成汇总；completion 顺序是竞态，这一帧只描述批次本身。",
    frame: pick(success, "stdout", "tool.updated/batch", isToolUpdated("batch")),
  },
  {
    case: "session-event-turn-completed",
    direction: "runtime->host",
    note: "终态事件：resultType / response / usage 都在这里。",
    frame: pick(success, "stdout", "turn.completed", isEvent("turn.completed")),
  },
  {
    case: "request-session-create",
    direction: "host->runtime",
    note: "host 建会话的请求（命令封套的实测形状）：workspace + 显式 model + 关掉标题生成。",
    frame: pick(
      success,
      "stdin",
      "session/create 请求",
      (frame) => frame.method === "session/create",
    ),
  },
  {
    case: "request-session-send",
    direction: "host->runtime",
    note: "host 提交输入的请求：sessionId + content。",
    frame: pick(success, "stdin", "session/send 请求", (frame) => frame.method === "session/send"),
  },
  {
    case: "response-runtime-preferences",
    direction: "host->runtime",
    note: "host 对反向请求的应答（镜像方向：Rust host 将来要发出这一帧）。",
    frame: pick(
      success,
      "stdin",
      "对 session/requestRuntimePreferences 的应答",
      (frame) => frame.result !== undefined,
    ),
  },
];

// 自描述守卫：应答帧（含错误应答）必须带上「应答哪条请求」，否则两端无从知道该用哪个
// result/error schema。关系由 id 从捕获推出，不手写。
for (const fixture of fixtures) {
  if (fixture.respondsTo === undefined && fixture.frame.method === undefined) {
    const respondsTo = requestMethodById(success, fixture.frame.id);
    if (respondsTo === undefined) {
      throw new Error(`${fixture.case}：应答帧没有解析出 respondsTo，夹具不自描述`);
    }
    fixture.respondsTo = respondsTo;
  }
}

await mkdir(FIXTURE_DIR, { recursive: true });
// 先清掉这个目录里已有的 json：夹具集就是下面这张表，重新录制不该留下上一次的孤儿文件
// （目录是这套协议夹具专用的；有孤儿时两端的「覆盖清单」断言本来也会红，这里让它更早、更明确）。
for (const stale of await readdir(FIXTURE_DIR)) {
  if (stale.endsWith(".json")) await unlink(join(FIXTURE_DIR, stale));
}
for (const fixture of fixtures) {
  await writeFile(
    join(FIXTURE_DIR, `${fixture.case}.json`),
    `${JSON.stringify(fixture, null, 2)}\n`,
    "utf8",
  );
  process.stdout.write(
    `录制 ${fixture.case}：${fixture.frame.method ?? "(应答)"} ${JSON.stringify(fixture.frame).length}B\n`,
  );
}
process.stdout.write(`\n共 ${fixtures.length} 条协议夹具 → ${FIXTURE_DIR}\n`);
