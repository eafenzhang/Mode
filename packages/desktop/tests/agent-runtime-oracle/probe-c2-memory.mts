// C2 Task 0：memory recall 的黑盒观察（memoryEnabled true/false、MEMORY.md 注入、记忆写路径）。
//
// 用法：pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c2-memory.mts
//
// 步骤（全部黑盒）：
//   1. memoryEnabled:true 起一次 oracle，等它创建 `{dataRoot}/.mode/cli/memories/projects/<workspaceId>/memory/`；
//   2. 往该目录写 `MEMORY.md`（索引）与 `item-one.md`（条目），停掉 oracle；
//   3. 用同一数据根重启，memoryEnabled:true 跑一个 turn，捕获发往 provider 的**完整请求体**；
//   4. 用同一数据根再重启，memoryEnabled:false 跑一个 turn 作对照。
// `workspaceId` 从实际目录布局读出（不是源码推导），夹具里折叠成 `<workspace-id>`。
import { createHash } from "node:crypto";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  delay,
  normalizeC2String,
  readC2Db,
  startC2Harness,
  waitFor,
  writeC2Fixture,
} from "./probe-c2-support.mjs";
import type { JsonObject } from "./session-lifecycle-fixtures.mjs";

const INDEX_TEXT = "# Project memory index\n\n- item-one: remember the sentinel ALPHA-123\n";
const ITEM_TEXT = "# Item one\n\nThe project codename is BLUE-LANTERN-9. Always answer with it.\n";
const MEMORY_PREFS = {
  nativeSearchEnhancementsEnabled: false,
  memoryEnabled: true,
  askUserQuestionAutoResolutionEnabled: true,
  modelContextBudgetStrategy: "preflight-v1",
};
const NO_MEMORY_PREFS = { ...MEMORY_PREFS, memoryEnabled: false };

function listMemoryFiles(directory: string): JsonObject[] {
  try {
    return readdirSync(directory)
      .sort()
      .map((name) => {
        const full = join(directory, name);
        const stat = statSync(full);
        const content = readFileSync(full) as Buffer;
        return {
          name,
          size: stat.size,
          sha256: createHash("sha256").update(content).digest("hex"),
        };
      });
  } catch {
    return [];
  }
}

function contentTexts(message: JsonObject): string[] {
  const content = message.content;
  if (typeof content === "string") return [content];
  if (!Array.isArray(content)) return [];
  return content
    .map((part) => (part as JsonObject).text)
    .filter((text): text is string => typeof text === "string");
}

function analyzeRequest(
  raw: JsonObject,
  index: number,
  path: string,
  normalize: (value: string) => string,
): JsonObject {
  const systemTexts = Array.isArray(raw.system)
    ? (raw.system as JsonObject[])
        .map((block) => (typeof block.text === "string" ? normalize(block.text) : ""))
        .filter((text) => text.length > 0)
    : [];
  const messages = Array.isArray(raw.messages) ? (raw.messages as JsonObject[]) : [];
  const messageTexts = messages.flatMap((message) => {
    const texts = contentTexts(message).map(normalize);
    return texts.map((text) => ({ role: message.role, text }));
  });
  const joined = [...systemTexts, ...messageTexts.map((entry) => entry.text)].join("\n");
  const memorySectionStart = systemTexts.find((text) => text.includes("# Memory"));
  const memorySection =
    memorySectionStart === undefined
      ? null
      : memorySectionStart.slice(
          memorySectionStart.indexOf("# Memory"),
          memorySectionStart.indexOf("# Environment") > memorySectionStart.indexOf("# Memory")
            ? memorySectionStart.indexOf("# Environment")
            : undefined,
        );
  const memoryFileBlock =
    messageTexts.find((entry) =>
      entry.text.includes("(user's auto-memory, persists across conversations)"),
    )?.text ?? null;
  const extractionInstruction =
    messageTexts.find((entry) => entry.text.includes("memory extraction subagent"))?.text ?? null;
  const existingFilesText = messageTexts.find((entry) =>
    entry.text.includes("## Existing memory files"),
  )?.text;
  const existingFilesBlock =
    existingFilesText === undefined
      ? null
      : existingFilesText.slice(existingFilesText.indexOf("## Existing memory files"));
  const tools = Array.isArray(raw.tools)
    ? (raw.tools as JsonObject[]).map((tool) => tool.name)
    : [];
  return {
    index,
    path,
    kind: extractionInstruction === null ? "main" : "memory-extraction",
    model: raw.model,
    max_tokens: raw.max_tokens,
    stream: raw.stream,
    toolCount: tools.length,
    toolNames: tools,
    systemTexts: systemTexts.map((text) => ({ chars: text.length, head: text.slice(0, 120) })),
    messageRoles: messages.map((message) => message.role),
    messageCount: messages.length,
    memorySection,
    memoryFileBlock,
    extractionInstruction,
    existingFilesBlock,
    containsIndexSentinel: joined.includes("ALPHA-123"),
    containsItemSentinel: joined.includes("BLUE-LANTERN-9"),
  };
}

async function warmup(dataRoot: string, cwd: string): Promise<JsonObject> {
  const harness = await startC2Harness({
    prefix: "memory-warmup",
    dataRoot,
    cwd,
    script: [{ text: "warmup answer" }],
    runtimePreferencesReply: MEMORY_PREFS,
  });
  try {
    await harness.sendAndWait("warmup prompt");
    const db = readC2Db(harness.dbPath, harness.context);
    const sessionRow = ((db.session as JsonObject).rows as JsonObject[])[0] ?? {};
    return {
      isolation: harness.isolation(),
      sessionWorkspaceId: sessionRow.workspace_id ?? null,
      sessionProjectId: sessionRow.project_id ?? null,
      sessionDirectory: sessionRow.directory ?? null,
    };
  } finally {
    await harness.stop();
  }
}

const dataRoot = await mkdtemp(join(tmpdir(), "mode-c2-memory-"));
const cwd = await mkdtemp(join(tmpdir(), "mode-c2-memory-ws-"));
const memoryRoot = join(dataRoot, ".mode", "cli", "memories", "projects");
try {
  const warm = await warmup(dataRoot, cwd);
  const workspaceIds = readdirSync(memoryRoot).filter((name) =>
    statSync(join(memoryRoot, name)).isDirectory(),
  );
  if (workspaceIds.length !== 1) {
    throw new Error(`预期一个 memory workspace 目录，实际 ${JSON.stringify(workspaceIds)}`);
  }
  const workspaceId = workspaceIds[0] as string;
  const memoryDir = join(memoryRoot, workspaceId, "memory");
  await mkdir(memoryDir, { recursive: true });
  await writeFile(join(memoryDir, "MEMORY.md"), INDEX_TEXT, "utf8");
  await writeFile(join(memoryDir, "item-one.md"), ITEM_TEXT, "utf8");
  const workspaceShape = /^(.*)-([0-9a-f]{16})$/u.exec(workspaceId);
  const filesAfterWrite = listMemoryFiles(memoryDir);

  // 重启让运行时在 materialization 时读到刚写入的记忆文件。
  const withMemory = await startC2Harness({
    prefix: "memory-on",
    dataRoot,
    cwd,
    script: [{ text: "memory turn answer" }],
    runtimePreferencesReply: MEMORY_PREFS,
  });
  let withMemoryRequests: JsonObject[];
  let withMemoryIsolation: JsonObject;
  let withMemoryEvents: string[];
  let withMemoryFiles: JsonObject[];
  let extractionObserved = false;
  try {
    await withMemory.sendAndWait("probe with memory");
    try {
      await waitFor(() => withMemory.rawBodies.length >= 2, "memory extraction request", 20_000);
      extractionObserved = true;
    } catch {
      extractionObserved = false;
    }
    await delay(800);
    withMemoryRequests = withMemory.rawBodies.map((raw, index) =>
      analyzeRequest(raw, index, "/v1/messages", (value) =>
        normalizeC2String(value, withMemory.context, {
          [workspaceId]: "<workspace-id>",
          [cwd]: "<cwd>",
          [dataRoot]: "<data-root>",
        }),
      ),
    );
    withMemoryIsolation = withMemory.isolation();
    withMemoryEvents = withMemory.captureState.events.map((event) => event.type);
    withMemoryFiles = listMemoryFiles(memoryDir);
  } finally {
    await withMemory.stop();
  }

  const withoutMemory = await startC2Harness({
    prefix: "memory-off",
    dataRoot,
    cwd,
    script: [{ text: "no memory turn answer" }],
    runtimePreferencesReply: NO_MEMORY_PREFS,
  });
  let withoutMemoryRequests: JsonObject[];
  let withoutMemoryIsolation: JsonObject;
  let withoutMemoryEvents: string[];
  try {
    await withoutMemory.sendAndWait("probe without memory");
    await delay(1_000);
    withoutMemoryRequests = withoutMemory.rawBodies.map((raw, index) =>
      analyzeRequest(raw, index, "/v1/messages", (value) =>
        normalizeC2String(value, withoutMemory.context, {
          [workspaceId]: "<workspace-id>",
          [cwd]: "<cwd>",
          [dataRoot]: "<data-root>",
        }),
      ),
    );
    withoutMemoryIsolation = withoutMemory.isolation();
    withoutMemoryEvents = withoutMemory.captureState.events.map((event) => event.type);
  } finally {
    await withoutMemory.stop();
  }

  const fixture = {
    case: "memory-recall",
    note:
      "C2 Task 0 oracle 捕获：memoryEnabled:true 时 MEMORY.md 索引注入首个 user 消息的 agentsMd " +
      "system-reminder，且系统提示含独立 # Memory 段；item-one.md 不进入主请求，只出现在" +
      "记忆抽取子代理请求的 Existing memory files 列表里。memoryEnabled:false 作对照。" +
      "workspaceId 从实际目录布局观察（<slug>-<16 hex>），夹具折叠为 <workspace-id>。",
    input: {
      memoryFiles: { "MEMORY.md": INDEX_TEXT, "item-one.md": ITEM_TEXT },
      memoryPreferences: MEMORY_PREFS,
      noMemoryPreferences: NO_MEMORY_PREFS,
      prompts: ["warmup prompt", "probe with memory", "probe without memory"],
    },
    expect: {
      warmup: warm,
      memoryRoot: "<data-root>/.mode/cli/memories/projects/<workspace-id>/memory",
      workspaceId: {
        matchesSlugHash: workspaceShape !== null,
        hashLength: workspaceShape?.[2]?.length ?? null,
        slugEqualsWorkspaceBasenameCaseInsensitive:
          workspaceShape?.[1]?.toLowerCase() === cwd.split(/[\\/]/u).at(-1)?.toLowerCase(),
        slugLowercased: workspaceShape?.[1] === workspaceShape?.[1]?.toLowerCase(),
      },
      filesAfterWrite,
      withMemory: {
        isolation: withMemoryIsolation,
        events: withMemoryEvents,
        requests: withMemoryRequests,
        extractionObserved,
        filesAfterTurn: withMemoryFiles,
      },
      withoutMemory: {
        isolation: withoutMemoryIsolation,
        events: withoutMemoryEvents,
        requests: withoutMemoryRequests,
      },
    },
  };
  const target = await writeC2Fixture("memory-recall", fixture);
  process.stdout.write(
    `${JSON.stringify(
      {
        fixture: target,
        workspaceId: {
          raw: workspaceId,
          shape: fixture.expect.workspaceId,
        },
        warmup: warm,
        filesAfterWrite,
        withMemory: withMemoryRequests.map((request) => ({
          index: request.index,
          kind: request.kind,
          toolCount: request.toolCount,
          toolNames: request.toolNames,
          messageRoles: request.messageRoles,
          memorySectionChars: (request.memorySection as string | null)?.length ?? null,
          memoryFileBlock:
            typeof request.memoryFileBlock === "string"
              ? request.memoryFileBlock.slice(0, 500)
              : null,
          extractionInstruction:
            typeof request.extractionInstruction === "string"
              ? request.extractionInstruction.slice(0, 300)
              : null,
          containsIndexSentinel: request.containsIndexSentinel,
          containsItemSentinel: request.containsItemSentinel,
        })),
        withoutMemory: withoutMemoryRequests.map((request) => ({
          index: request.index,
          kind: request.kind,
          containsIndexSentinel: request.containsIndexSentinel,
          containsItemSentinel: request.containsItemSentinel,
        })),
        extractionObserved,
        filesAfterTurn: withMemoryFiles,
      },
      null,
      2,
    )}\n`,
  );
} finally {
  if (!process.argv.includes("--keep")) {
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
}
