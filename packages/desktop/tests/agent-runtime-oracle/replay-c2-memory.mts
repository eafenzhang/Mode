// C2 Task 8：memory 召回对拍（`memory-recall` 夹具；`replay-c2.mts` 的 memory 半边）。
//
// 与 compact 回放不同，注入面只能从 **provider 请求体**观察：本 driver 用与 oracle
// 录制时同一形状的本地 mock provider（`mock-provider.mts` + `MODE_PERSONAL_PROVIDER_CONFIG_FILE`
// 指向隔离根内的配置），对 Rust `agent-server` 的 anthropic-messages wire 请求做投影。
// 比较面（契约 §3.5 登记的 Rust 可接受子集）：
//
// - `# Memory` 系统段：Rust 系统文本**逐字包含**夹具录制的 `memorySection`
//   （路径折叠位换真实目录后做 `normalizeC2String` 两侧同折）；
// - agentsMd 块：首条 user 文本**逐字包含**夹具录制的 `memoryFileBlock`
//   （currentDate 的日期格式未观察，夹具折叠 `<date>`，Rust 取 UTC 日历日——
//   登记的待观察假设，比较前同折）；
// - 哨兵：索引 `ALPHA-123` 出现、item `BLUE-LANTERN-9` 不出现（索引优先，§3.2）；
// - enabled 只有 **1 个**模型请求（写路径/extraction 未实现——登记 §3.5）；
// - 事件类型序列（登记缺省同删后）：turn.started / model.streaming / turn.completed。
//
// 登记**不比较**（§3.5 差异）：wire roles/parts 形状（Rust 保持 C1 的
// [System, User] 装配，oracle wire 是 ["user","system"] + 双 text part）；
// `toolCount`/`toolNames`（随环境变化，§1.4/§4.2 折叠口径）。
//
// 对照组（夹具同款）：`withoutMemory`（同一数据根 + 关闭 → 零注入）与
// `withoutMemoryFreshRoot`（全新根 + 关闭 → 不创建 `memories/`）。oracle 在
// enabled 时会创建记忆目录；Rust 只读不创建（登记偏差）——本 driver 按 Rust 语义
// 自行落盘夹具 `input.memoryFiles`（workspaceId 目录名形状 `<slug>-<16 hex>`，
// hash 算法未观察，Rust 按「恰好一个命中」扫描发现）。
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";

import { readSessionColumn, normalizeC2String } from "./probe-c2-support.mjs";
import type { MockStep } from "./mock-provider.mjs";
import {
  C2_ABSENT_EVENT_TYPES,
  diffDeep,
  sendTurnAndWait,
  startC2Replay,
  stopC2Replay,
  type C2Diff,
  type C2Fixture,
  type C2ReplayResult,
  type C2Run,
} from "./replay-c2.mjs";
import type { JsonObject } from "./session-lifecycle-fixtures.mjs";

const DEFAULT_PREFS = {
  nativeSearchEnhancementsEnabled: false,
  memoryEnabled: false,
  askUserQuestionAutoResolutionEnabled: true,
  modelContextBudgetStrategy: "preflight-v1",
};
const MEMORY_PREFS = { ...DEFAULT_PREFS, memoryEnabled: true };

type MemoryHarness = {
  run: C2Run;
  rawBodies: JsonObject[];
  mockPaths: string[];
  /** 只停服务与 mock（保留隔离目录；「同一数据根重启」流程用）。 */
  stopSession(): Promise<void>;
  /** 删除隔离目录（调用方负责在流程结束后调用；owns=false 时也不删）。 */
  owns: boolean;
  dataRoot: string;
  cwd: string;
};

/** 起 memory 对拍运行：本地 mock provider（真实 hub 装配；请求体经 run.rawBodies 观测）。 */
async function startMemoryHarness(options: {
  fixture: C2Fixture;
  caseLabel: string;
  dataRoot?: string;
  cwd?: string;
  memoryEnabled: boolean;
  script: MockStep[];
}): Promise<MemoryHarness> {
  const prefix = `mode-c2-${options.caseLabel}-`;
  const dataRoot = options.dataRoot ?? (await mkdtemp(join(tmpdir(), prefix)));
  const cwd = options.cwd ?? (await mkdtemp(join(tmpdir(), `${prefix}ws-`)));
  const run = await startC2Replay(
    { ...options.fixture, case: options.caseLabel },
    {
      mockScript: options.script,
      runtimePreferences: options.memoryEnabled ? MEMORY_PREFS : DEFAULT_PREFS,
    },
  );
  // startC2Replay 自建 dataRoot/cwd；重启流程传入已存在的目录时，目录句柄以
  // harness 侧为准（run 上的只是同一值）。
  return {
    run,
    rawBodies: run.rawBodies,
    mockPaths: run.mockRequestPaths,
    owns: options.dataRoot === undefined && options.cwd === undefined,
    dataRoot,
    cwd,
    async stopSession() {
      await stopC2Replay(run);
    },
  };
}

/** 删除 harness 的隔离目录（owns=false 的重启场景跳过）。 */
async function removeHarnessDirs(harness: MemoryHarness): Promise<void> {
  if (!harness.owns) return;
  await rm(harness.dataRoot, { recursive: true, force: true });
  await rm(harness.cwd, { recursive: true, force: true });
}

/** 与 probe-c2-memory.mts 同款的请求投影（注入面字段；roles/parts 不投影——登记偏差）。 */
function analyzeRequest(raw: JsonObject, normalize: (value: string) => string): JsonObject {
  const systemTexts = Array.isArray(raw.system)
    ? (raw.system as JsonObject[])
        .map((block) => (typeof block.text === "string" ? normalize(block.text) : ""))
        .filter((text) => text.length > 0)
    : [];
  const messages = Array.isArray(raw.messages) ? (raw.messages as JsonObject[]) : [];
  const messageTexts = messages.flatMap((message) => {
    const content = message.content;
    const texts =
      typeof content === "string"
        ? [content]
        : Array.isArray(content)
          ? content
              .map((part) => (part as JsonObject).text)
              .filter((text): text is string => typeof text === "string")
          : [];
    return texts.map(normalize);
  });
  const joined = [...systemTexts, ...messageTexts].join("\n");
  const memoryBlockIndex = systemTexts.findIndex((text) => text.includes("# Memory"));
  const memorySection =
    memoryBlockIndex < 0
      ? null
      : sectionBetween(systemTexts[memoryBlockIndex] as string, "# Memory", "# Environment");
  const memoryFileBlock =
    messageTexts.find((text) =>
      text.includes("(user's auto-memory, persists across conversations)"),
    ) ?? null;
  return {
    memorySection,
    memoryFileBlock,
    containsIndexSentinel: joined.includes("ALPHA-123"),
    containsItemSentinel: joined.includes("BLUE-LANTERN-9"),
  };
}

/** 取 `start` 标记起到 `end` 标记（或文本末尾）的段（与探针的 memorySection 切片同规则）。 */
function sectionBetween(text: string, start: string, end: string): string {
  const startIndex = text.indexOf(start);
  if (startIndex < 0) return "";
  const endIndex = text.indexOf(end, startIndex);
  return endIndex >= 0 ? text.slice(startIndex, endIndex) : text.slice(startIndex);
}

/** workspaceId 目录（Rust 扫描形状 `<slug>-<16 hex>`；hash 未观察，形状自拟）。 */
function workspaceIdFor(cwd: string): string {
  return `${basename(cwd).toLowerCase()}-0123456789abcdef`;
}

/** 落盘夹具 `input.memoryFiles`（Rust 不创建目录——登记偏差；目录由本 harness 建）。 */
async function seedMemoryFiles(fixture: C2Fixture, cwd: string, dataRoot: string): Promise<string> {
  const memoryDir = join(
    dataRoot,
    ".mode",
    "cli",
    "memories",
    "projects",
    workspaceIdFor(cwd),
    "memory",
  );
  await mkdir(memoryDir, { recursive: true });
  const files = (fixture.input.memoryFiles ?? {}) as Record<string, string>;
  for (const [name, content] of Object.entries(files)) {
    await writeFile(join(memoryDir, name), content, "utf8");
  }
  return memoryDir;
}

/** normalize 的 extra 折叠位：workspaceId → `<workspace-id>`（与夹具同词）。 */
function normalizeFor(harness: MemoryHarness): (value: string) => string {
  return (value: string) =>
    normalizeC2String(value, harness.run.context, {
      [workspaceIdFor(harness.run.cwd)]: "<workspace-id>",
    });
}

/** 事件类型序列（登记缺省同删后；memory 夹具只录了类型序列）。 */
function filteredEventTypes(run: C2Run): string[] {
  return run.frames()
    .map((event) => event.type)
    .filter((type) => !C2_ABSENT_EVENT_TYPES.includes(type));
}

/** 主对拍：enabled 注入面（夹具 withMemory）。 */
export async function replayMemoryRecall(fixture: C2Fixture): Promise<C2ReplayResult> {
  const diffs: C2Diff[] = [];
  const notes: string[] = [];
  const expect = fixture.expect as JsonObject;
  const enabled = await startMemoryHarness({
    fixture,
    caseLabel: "memory-recall",
    memoryEnabled: true,
    script: [{ text: "memory turn answer" }],
  });
  try {
    await seedMemoryFiles(fixture, enabled.run.cwd, enabled.run.dataRoot);
    await sendTurnAndWait(enabled.run, "probe with memory");
    const normalized = analyzeRequest(enabled.rawBodies[0] ?? {}, normalizeFor(enabled));
    const expectRequest = ((expect.withMemory as JsonObject)?.requests as JsonObject[])?.[0] ?? {};
    // `# Memory` 段：夹具录制文本逐字包含（路径折叠后）。
    const expectedSection = (expectRequest.memorySection as string | null) ?? "";
    if (expectedSection === "") {
      diffs.push({ layer: "response", path: "withMemory.memorySection", message: "夹具没有录制 # Memory 段", actual: normalized.memorySection, expected: null });
    } else if (
      typeof normalized.memorySection !== "string" ||
      !normalized.memorySection.includes(expectedSection)
    ) {
      diffs.push({
        layer: "response",
        path: "withMemory.memorySection",
        message: "系统文本必须逐字包含夹具录制的 # Memory 段",
        actual: normalized.memorySection?.slice(0, 400) ?? null,
        expected: expectedSection.slice(0, 400),
      });
    }
    // agentsMd 块：夹具录制文本逐字包含（日期折叠后；<date> 的格式是登记假设）。
    const expectedBlock = (expectRequest.memoryFileBlock as string | null) ?? "";
    if (expectedBlock === "") {
      diffs.push({ layer: "response", path: "withMemory.memoryFileBlock", message: "夹具没有录制 agentsMd 块", actual: normalized.memoryFileBlock, expected: null });
    } else if (
      typeof normalized.memoryFileBlock !== "string" ||
      !normalized.memoryFileBlock.includes(expectedBlock)
    ) {
      diffs.push({
        layer: "response",
        path: "withMemory.memoryFileBlock",
        message: "首条 user 文本必须逐字包含夹具录制的 agentsMd 块",
        actual: normalized.memoryFileBlock?.slice(0, 400) ?? null,
        expected: expectedBlock.slice(0, 400),
      });
    }
    diffs.push(...diffDeep("response", "withMemory.containsIndexSentinel", normalized.containsIndexSentinel, expectRequest.containsIndexSentinel));
    diffs.push(...diffDeep("response", "withMemory.containsItemSentinel", normalized.containsItemSentinel, expectRequest.containsItemSentinel));
    // enabled 只有 1 个模型请求（extraction 写路径未实现——登记 §3.5；夹具录了 2 个）。
    notes.push(
      `模型请求数 ${enabled.rawBodies.length}（夹具录 2：主请求 + extraction；extraction 未实现，登记 §3.5）`,
    );
    diffs.push(...diffDeep("rows", "withMemory.requestCount", enabled.rawBodies.length, 1));
    // 事件类型序列（登记缺省同删后）。
    const expectedTypes = ((expect.withMemory as JsonObject)?.events as string[]) ?? [];
    diffs.push(
      ...diffDeep(
        "events",
        "withMemory.eventTypes",
        filteredEventTypes(enabled.run),
        expectedTypes.filter((type) => !C2_ABSENT_EVENT_TYPES.includes(type)),
      ),
    );
    // `time_compacting` 观察值恒 null（§1.3）对 prompt turn 同样成立。
    diffs.push(
      ...diffDeep(
        "rows",
        "withMemory.timeCompacting",
        String(readSessionColumn(enabled.run.dbPath, "time_compacting") ?? "null"),
        "null",
      ),
    );
    return { case: fixture.case, diffs, notes, stderr: enabled.run.stderr };
  } finally {
    await enabled.stopSession();
    await removeHarnessDirs(enabled);
  }
}

/** 关闭开关：同一数据根 + 同一记忆文件 → 零注入、单请求（§3.4 withoutMemory）。 */
export async function replayMemoryDisabled(fixture: C2Fixture): Promise<C2ReplayResult> {
  const diffs: C2Diff[] = [];
  const notes: string[] = [];
  const expect = fixture.expect as JsonObject;
  const first = await startMemoryHarness({
    fixture,
    caseLabel: "memory-recall-off",
    memoryEnabled: true,
    script: [{ text: "warmup answer" }],
  });
  try {
    await seedMemoryFiles(fixture, first.run.cwd, first.run.dataRoot);
    // 只停服务（保留数据根/工作目录）：关闭对照组要在同一隔离根上重启。
    await first.stopSession();
    const disabled = await startMemoryHarness({
      fixture,
      caseLabel: "memory-recall-off",
      dataRoot: first.dataRoot,
      cwd: first.cwd,
      memoryEnabled: false,
      script: [{ text: "no memory turn answer" }],
    });
    try {
      await sendTurnAndWait(disabled.run, "probe without memory");
      const normalized = analyzeRequest(disabled.rawBodies[0] ?? {}, normalizeFor(disabled));
      const expectRequest = ((expect.withoutMemory as JsonObject)?.requests as JsonObject[])?.[0] ?? {};
      diffs.push(...diffDeep("response", "withoutMemory.memorySection", normalized.memorySection, expectRequest.memorySection ?? null));
      diffs.push(...diffDeep("response", "withoutMemory.memoryFileBlock", normalized.memoryFileBlock, expectRequest.memoryFileBlock ?? null));
      diffs.push(...diffDeep("response", "withoutMemory.containsIndexSentinel", normalized.containsIndexSentinel, false));
      diffs.push(...diffDeep("response", "withoutMemory.containsItemSentinel", normalized.containsItemSentinel, false));
      diffs.push(...diffDeep("rows", "withoutMemory.requestCount", disabled.rawBodies.length, 1));
      return { case: "memory-recall:disabled", diffs, notes, stderr: disabled.run.stderr };
    } finally {
      await disabled.stopSession();
      await removeHarnessDirs(first);
    }
  } finally {
    void expect;
  }
}

/** 全新根 + 关闭：不创建 `memories/`（§3.1 withoutMemoryFreshRoot）。 */
export async function replayMemoryFreshDisabled(fixture: C2Fixture): Promise<C2ReplayResult> {
  const diffs: C2Diff[] = [];
  const notes: string[] = [];
  const expect = fixture.expect as JsonObject;
  const fresh = await startMemoryHarness({
    fixture,
    caseLabel: "memory-recall-fresh",
    memoryEnabled: false,
    script: [{ text: "fresh no-memory turn answer" }],
  });
  try {
    await sendTurnAndWait(fresh.run, "fresh root without memory");
    const memoriesRoot = join(fresh.run.dataRoot, ".mode", "cli", "memories");
    const expectBlock = (expect.withoutMemoryFreshRoot as JsonObject) ?? {};
    diffs.push(...diffDeep("rows", "withoutMemoryFreshRoot.memoriesRootExists", existsSync(memoriesRoot), expectBlock.memoriesRootExists ?? false));
    const normalized = analyzeRequest(fresh.rawBodies[0] ?? {}, normalizeFor(fresh));
    diffs.push(...diffDeep("response", "withoutMemoryFreshRoot.memorySection", normalized.memorySection, null));
    diffs.push(...diffDeep("response", "withoutMemoryFreshRoot.memoryFileBlock", normalized.memoryFileBlock, null));
    return { case: "memory-recall:fresh-disabled", diffs, notes, stderr: fresh.run.stderr };
  } finally {
    await fresh.stopSession();
    await removeHarnessDirs(fresh);
  }
}

// 注册：memory-recall 夹具拆三条套件用例（主对拍 + 两组对照）。
export function registerMemoryCases(registry: Record<string, (fixture: C2Fixture) => Promise<C2ReplayResult>>): void {
  registry["memory-recall"] = replayMemoryRecall;
  registry["memory-recall:disabled"] = replayMemoryDisabled;
  registry["memory-recall:fresh-disabled"] = replayMemoryFreshDisabled;
}
