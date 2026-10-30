// M2 Task 1：共享协议夹具的双端断言（TS 侧）。
//
// 规格：`docs/specs/tauri-agent-runtime.md` §1.2（stdio 的实测线上形状）。
// 镜像面（ledger Ruling 13）：旧运行时在 stdio 上实际说的话——方法名 JSON-RPC 帧
// （通知 / 反向请求 / 应答 / 错误应答）+ `session/event` 上的会话事件信封；
// **不是** `mode-protocol-v4/**` 的主题帧（v4 是 host 面，运行时不在这条链路上说它）。
//
// 本文件与 Rust 侧 `crates/agent-protocol/tests/fixtures.rs` **读同一个目录**
// `packages/shared/test/fixtures/agent-protocol/`：同一份 JSON，这里用 `mode-protocol`（v1）
// 的真实 zod schema 校验，那边用 serde 镜像类型校验。哪一端单方面改了类型而另一端没跟上，
// 那一端的用例就会红——这就是本任务的全部意义。
//
// 夹具不是照着镜像写的：它由 `record-protocol-fixtures.mts` 从 oracle 的 stdout/stdin 上
// **整帧原样**录下（见该文件的纪律说明）。
// 例外：`request-session-list` / `response-session-list` 目前是**按协议声明 schema 构造**的
// 形状夹具（note 已注明「非录制帧」），等会话只读链路接上运行时后补录制版本。
import { test } from "node:test";
import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import {
  modeMcpListParamsSchema,
  modeMcpListResultSchema,
  modeProcessChildProcessesResultSchema,
  modePluginsDescribeParamsSchema,
  modePluginsDescribeResultSchema,
  modePluginsListParamsSchema,
  modePluginsListResultSchema,
  modePluginsOverviewParamsSchema,
  modePluginsOverviewResultSchema,
  modeProtocolMessageSchema,
  modeSessionCloseParamsSchema,
  modeSessionCloseResultSchema,
  modeSessionCreateParamsSchema,
  modeSessionEventSchema,
  modeSessionEventsParamsSchema,
  modeSessionEventsResultSchema,
  modeSessionListParamsSchema,
  modeSessionListResultSchema,
  modeSessionMessagesParamsSchema,
  modeSessionMessagesResultSchema,
  modeSessionReadParamsSchema,
  modeSessionRequestRuntimePreferencesParamsSchema,
  modeSessionRuntimePreferencesResultSchema,
  modeSessionSendParamsSchema,
  modeSessionSendResultSchema,
  modeSessionSetModeParamsSchema,
  modeSessionSetModelParamsSchema,
  modeSessionSetThoughtLevelParamsSchema,
  modeSessionStateSnapshotSchema,
  modeSessionSubagentsParamsSchema,
  modeSessionSubagentsResultSchema,
  modeSessionSubscribeParamsSchema,
  modeSessionSubscribeResultSchema,
  modeWorkspacePresentationSchema,
  modeWorkspaceReadPresentationParamsSchema,
  modeWorkflowsGetParamsSchema,
  modeWorkflowsGetResultSchema,
  modeWorkflowsListParamsSchema,
  modeWorkflowsListResultSchema,
  modeWorkflowsRunsParamsSchema,
  modeWorkflowsRunsResultSchema,
  modeWorkflowsUpdateMetaParamsSchema,
  modeWorkflowsUpdateMetaResultSchema,
  modeWorkflowsDeleteParamsSchema,
  modeWorkflowsDeleteResultSchema,
  modeWorkflowsMoveParamsSchema,
  modeWorkflowsMoveResultSchema,
} from "../../../shared/src/mode-protocol/index.js";
import {
  sessionDebugParamsSchema,
  sessionDebugSnapshotSchema,
} from "../../../shared/src/session-debug.js";
import {
  commandAckSchema,
  commandEnvelopeSchema,
} from "../../../shared/src/mode-protocol-v4/command.js";
import {
  v4ConversationPlansParamsSchema,
  v4ConversationPlansResultSchema,
  v4ConversationRowsRangeParamsSchema,
  v4ConversationRowsRangeResultSchema,
  v4ConversationSubscribeParamsSchema,
  v4ConversationSubscribeResultSchema,
  v4ConversationWorkflowRunEventsParamsSchema,
  v4ConversationWorkflowRunEventsResultSchema,
  v4ConversationWorkflowRunsParamsSchema,
  v4ConversationWorkflowRunsResultSchema,
} from "../../../shared/src/mode-protocol-v4/transport.js";
import {
  v4ConversationWorkflowRunArtifactDataParamsSchema,
  v4ConversationWorkflowRunArtifactDataResultSchema,
  v4ConversationWorkflowRunArtifactReadParamsSchema,
  v4ConversationWorkflowRunArtifactReadResultSchema,
  v4ConversationWorkflowRunArtifactsParamsSchema,
  v4ConversationWorkflowRunArtifactsResultSchema,
} from "../../../shared/src/mode-protocol-v4/workflow-artifacts.js";
import {
  v4ConversationWorkflowRunNodeResultParamsSchema,
  v4ConversationWorkflowRunNodeResultResultSchema,
  v4ConversationWorkflowRunWorkspaceParamsSchema,
  v4ConversationWorkflowRunWorkspaceResultSchema,
} from "../../../shared/src/mode-protocol-v4/workflow-workspace.js";

/** 夹具外壳：`frame` 是线上整帧，其余是录制元数据（不属于线上形状）。 */
type ProtocolFixture = {
  case: string;
  direction: "runtime->host" | "host->runtime";
  respondsTo?: string;
  note: string;
  frame: Record<string, unknown>;
};

const FIXTURE_DIR = fileURLToPath(
  new URL("../../../shared/test/fixtures/agent-protocol/", import.meta.url),
);

/**
 * v1 共享 schema **落后于线上**的载荷字段（Task 1 实测，2026-10-16）。
 *
 * 后果不是「文档不准」，而是**线上正在丢事件**：宿主路径用 `modeSessionEventSchema.safeParse(...)`
 * 校验 `session/event`，解析失败就把整条事件记日志丢掉。载荷带 schema 未声明的键、
 * 且各成员是 `.strict()` —— `unrecognized_keys` 会拒绝整条事件。
 *
 * **两处滞后已于 2026-10-24 全部修复**（表清空，作为墓碑保留）：
 * - `turn.started` / `executionStartedAt`：schema 补声明浮点 epoch 毫秒可选键
 *   （回归 `packages/shared/test/mode-protocol-session-event-drift.test.ts`，
 *   端到端证据：stream-latency-host bench 的丢弃日志清零）；
 * - `tool.updated:started` / `readOnly`+`sideEffectScope`：schema 补声明
 *   布尔 + 7 值 scope 枚举（同一回归文件；端到端证据：本套夹具的 started 帧从
 *   「靠滞后表放行」变为全量 parse 通过）。
 *
 * 断言「被拒绝的键逐字等于表里的键」保留为**长期绊网**：表现在是空的，
 * 任何未来被 schema 拒绝的会话事件夹具都会直接红（提示：要么补镜像/补 schema，
 * 要么显式登记新滞后并说明后果）——别让「为什么有这张表」消失。
 */
const KNOWN_V1_PAYLOAD_LAG: Record<string, readonly string[]> = {};

/** 事件夹具的事件类型 → `KNOWN_V1_PAYLOAD_LAG` 的键。 */
function lagKeyOf(params: Record<string, unknown>): string {
  const type = String(params.type);
  if (type === "tool.updated") {
    const kind = (params.payload as { kind?: unknown } | undefined)?.kind;
    return `${type}:${String(kind)}`;
  }
  return type;
}

/** zod 在载荷层报出的未声明键（`unrecognized_keys`），用来核对滞后表。 */
function unrecognizedKeys(error: {
  issues: readonly { code: string; keys?: readonly string[] }[];
}): string[] {
  return error.issues
    .filter((issue) => issue.code === "unrecognized_keys")
    .flatMap((issue) => [...(issue.keys ?? [])])
    .sort();
}

/** 结构化的 zod 校验结果类型：避免为一个断言去 import zod 本体。 */
type SafeParseOutcome =
  | { success: true }
  | {
      success: false;
      error: { issues: { code: string; keys?: readonly string[]; message: string }[] };
    };
type Parseable = { safeParse: (value: unknown) => SafeParseOutcome };

/** 断言某个真实 schema 接受这份载荷；不接受就把 path/message 打出来。 */
function assertAccepted(schema: Parseable, value: unknown, what: string): void {
  const parsed = schema.safeParse(value);
  assert.ok(
    parsed.success,
    `${what} 不被 v1 schema 接受：${parsed.success ? "" : JSON.stringify(parsed.error.issues)}`,
  );
}

async function loadFixtures(): Promise<ProtocolFixture[]> {
  const entries = (await readdir(FIXTURE_DIR)).filter((name) => name.endsWith(".json")).sort();
  const fixtures: ProtocolFixture[] = [];
  for (const entry of entries) {
    fixtures.push(JSON.parse(await readFile(join(FIXTURE_DIR, entry), "utf8")) as ProtocolFixture);
  }
  return fixtures;
}

const fixtures = await loadFixtures();

test("夹具目录非空，且每条夹具都自描述（case / direction / frame）", () => {
  // 这条断言同时是「两端共读一个目录」的书面凭据：Rust 侧
  // `crates/agent-protocol/tests/fixtures.rs` 的 `fixture_dir()` 拼的是同一个相对路径。
  assert.ok(
    FIXTURE_DIR.replaceAll("\\", "/").endsWith("packages/shared/test/fixtures/agent-protocol/"),
    `夹具目录与 Rust 侧不一致：${FIXTURE_DIR}`,
  );
  assert.ok(fixtures.length > 0, "夹具目录是空的，这条用例等于没跑");
  const cases = new Set<string>();
  for (const fixture of fixtures) {
    assert.match(fixture.case, /^[a-z0-9-]+$/, `${fixture.case}：case 名要能当文件名`);
    assert.ok(!cases.has(fixture.case), `夹具 case 重复：${fixture.case}`);
    cases.add(fixture.case);
    assert.ok(
      fixture.direction === "runtime->host" || fixture.direction === "host->runtime",
      `${fixture.case}：direction 必须是两个方向之一`,
    );
    assert.ok(
      typeof fixture.note === "string" && fixture.note.length > 0,
      `${fixture.case} 缺 note`,
    );
    // 应答帧没有 method，必须靠 respondsTo 说清它在应答哪条请求（否则不知道该用哪张 result schema）。
    if (fixture.frame.method === undefined) {
      assert.ok(
        typeof fixture.respondsTo === "string" && fixture.respondsTo.length > 0,
        `${fixture.case}：应答帧必须带 respondsTo`,
      );
    }
  }
});

test("每条夹具的帧都能被 v1 协议消息 schema 接受", () => {
  for (const fixture of fixtures) {
    const parsed = modeProtocolMessageSchema.safeParse(fixture.frame);
    assert.ok(
      parsed.success,
      `${fixture.case} 的帧不是合法的 v1 协议消息：${parsed.success ? "" : JSON.stringify(parsed.error.issues)}`,
    );
  }
});

test("方向与帧形状一致：host->runtime 只出现请求与应答", () => {
  for (const fixture of fixtures) {
    const hasMethod = fixture.frame.method !== undefined;
    const hasResult = fixture.frame.result !== undefined;
    const hasError = fixture.frame.error !== undefined;
    if (fixture.direction === "host->runtime") {
      // 这条链路上 host 只会发请求、以及应答运行时的反向请求。
      assert.ok(
        (hasMethod || hasResult || hasError) && !(hasMethod && hasResult),
        `${fixture.case}：host 方向出现了非请求/非应答的帧`,
      );
    }
    if (fixture.direction === "runtime->host" && hasError) {
      assert.ok(
        typeof (fixture.frame.error as { code?: unknown }).code === "number",
        `${fixture.case}：错误应答必须带数字 code`,
      );
    }
  }
});

test("会话事件的载荷能被 v1 session event schema 接受，或只差已记录的滞后字段", () => {
  let eventFixtures = 0;
  const exercisedLagKeys = new Set<string>();
  for (const fixture of fixtures) {
    if (fixture.frame.method !== "session/event") continue;
    eventFixtures += 1;
    const params = fixture.frame.params as Record<string, unknown>;
    const parsed = modeSessionEventSchema.safeParse(params);
    if (parsed.success) continue;
    const lagKey = lagKeyOf(params);
    const declared = KNOWN_V1_PAYLOAD_LAG[lagKey];
    assert.ok(
      declared !== undefined,
      `${fixture.case}（${lagKey}）的载荷被 v1 schema 拒绝，但滞后表里没有它：` +
        `${JSON.stringify(parsed.error.issues)}。要么补镜像/补 schema，要么把滞后写进表里` +
        `（这张表的后果见 KNOWN_V1_PAYLOAD_LAG 的注释：宿主会丢掉这类事件）。`,
    );
    assert.deepEqual(
      unrecognizedKeys(parsed.error),
      [...declared].sort(),
      `${fixture.case}（${lagKey}）被拒绝的字段与滞后表不一致：schema 或运行时动了，两端要一起动`,
    );
    exercisedLagKeys.add(lagKey);
  }
  assert.ok(eventFixtures > 0, "没有会话事件夹具，这条用例等于没跑");
  assert.deepEqual(
    [...exercisedLagKeys].sort(),
    Object.keys(KNOWN_V1_PAYLOAD_LAG).sort(),
    "滞后表里有没被夹具覆盖的条目（或夹具覆盖到了未登记的滞后）",
  );
});

test("host 命令与应答的载荷能被对应的 v1 schema 接受", () => {
  const paramsSchemas: Record<string, Parseable> = {
    "mcp/list": modeMcpListParamsSchema,
    "plugins/list": modePluginsListParamsSchema,
    "plugins/describe": modePluginsDescribeParamsSchema,
    "plugins/overview": modePluginsOverviewParamsSchema,
    "session/close": modeSessionCloseParamsSchema,
    "session/create": modeSessionCreateParamsSchema,
    "session/debug": sessionDebugParamsSchema,
    "session/events": modeSessionEventsParamsSchema,
    "session/list": modeSessionListParamsSchema,
    "session/messages": modeSessionMessagesParamsSchema,
    "session/read": modeSessionReadParamsSchema,
    "session/send": modeSessionSendParamsSchema,
    "session/setMode": modeSessionSetModeParamsSchema,
    "session/setModel": modeSessionSetModelParamsSchema,
    "session/setThoughtLevel": modeSessionSetThoughtLevelParamsSchema,
    "session/subagents": modeSessionSubagentsParamsSchema,
    "session/subscribe": modeSessionSubscribeParamsSchema,
    "session/requestRuntimePreferences": modeSessionRequestRuntimePreferencesParamsSchema,
    "v4/command": commandEnvelopeSchema,
    "v4/conversation/plans": v4ConversationPlansParamsSchema,
    "v4/conversation/rowsRange": v4ConversationRowsRangeParamsSchema,
    "v4/conversation/subscribe": v4ConversationSubscribeParamsSchema,
    "v4/conversation/workflowRunEvents": v4ConversationWorkflowRunEventsParamsSchema,
    "v4/conversation/workflowRuns": v4ConversationWorkflowRunsParamsSchema,
    "v4/conversation/workflowRunArtifacts": v4ConversationWorkflowRunArtifactsParamsSchema,
    "v4/conversation/workflowRunArtifactData": v4ConversationWorkflowRunArtifactDataParamsSchema,
    "v4/conversation/workflowRunArtifactRead": v4ConversationWorkflowRunArtifactReadParamsSchema,
    "v4/conversation/workflowRunWorkspace": v4ConversationWorkflowRunWorkspaceParamsSchema,
    "v4/conversation/workflowRunNodeResult": v4ConversationWorkflowRunNodeResultParamsSchema,
    "workspace/readPresentation": modeWorkspaceReadPresentationParamsSchema,
    "workflows/get": modeWorkflowsGetParamsSchema,
    "workflows/list": modeWorkflowsListParamsSchema,
    "workflows/runs": modeWorkflowsRunsParamsSchema,
    "workflows/updateMeta": modeWorkflowsUpdateMetaParamsSchema,
    "workflows/delete": modeWorkflowsDeleteParamsSchema,
    "workflows/move": modeWorkflowsMoveParamsSchema,
  };
  const resultSchemas: Record<string, Parseable> = {
    "mcp/list": modeMcpListResultSchema,
    "plugins/list": modePluginsListResultSchema,
    "plugins/describe": modePluginsDescribeResultSchema,
    "plugins/overview": modePluginsOverviewResultSchema,
    "process/childProcesses": modeProcessChildProcessesResultSchema,
    "workflows/get": modeWorkflowsGetResultSchema,
    "workflows/list": modeWorkflowsListResultSchema,
    "workflows/runs": modeWorkflowsRunsResultSchema,
    "workflows/updateMeta": modeWorkflowsUpdateMetaResultSchema,
    "workflows/delete": modeWorkflowsDeleteResultSchema,
    "workflows/move": modeWorkflowsMoveResultSchema,
    "session/close": modeSessionCloseResultSchema,
    "session/debug": sessionDebugSnapshotSchema,
    "session/events": modeSessionEventsResultSchema,
    "session/list": modeSessionListResultSchema,
    "session/messages": modeSessionMessagesResultSchema,
    "session/read": modeSessionStateSnapshotSchema,
    "session/send": modeSessionSendResultSchema,
    "session/setMode": modeSessionStateSnapshotSchema,
    "session/setModel": modeSessionStateSnapshotSchema,
    "session/setThoughtLevel": modeSessionStateSnapshotSchema,
    "session/subagents": modeSessionSubagentsResultSchema,
    "session/subscribe": modeSessionSubscribeResultSchema,
    "session/requestRuntimePreferences": modeSessionRuntimePreferencesResultSchema,
    "v4/command": commandAckSchema,
    "v4/conversation/plans": v4ConversationPlansResultSchema,
    "v4/conversation/rowsRange": v4ConversationRowsRangeResultSchema,
    "v4/conversation/subscribe": v4ConversationSubscribeResultSchema,
    "v4/conversation/workflowRunEvents": v4ConversationWorkflowRunEventsResultSchema,
    "v4/conversation/workflowRuns": v4ConversationWorkflowRunsResultSchema,
    "v4/conversation/workflowRunArtifacts": v4ConversationWorkflowRunArtifactsResultSchema,
    "v4/conversation/workflowRunArtifactData": v4ConversationWorkflowRunArtifactDataResultSchema,
    "v4/conversation/workflowRunArtifactRead": v4ConversationWorkflowRunArtifactReadResultSchema,
    "v4/conversation/workflowRunWorkspace": v4ConversationWorkflowRunWorkspaceResultSchema,
    "v4/conversation/workflowRunNodeResult": v4ConversationWorkflowRunNodeResultResultSchema,
    "workspace/readPresentation": modeWorkspacePresentationSchema,
  };
  const exercisedCommands = new Set<string>();
  for (const fixture of fixtures) {
    const method = fixture.frame.method as string | undefined;
    // 只有**请求**（带 id）的参数才有对应的 params schema；通知（无 id）走上面的事件用例。
    if (
      method !== undefined &&
      fixture.frame.id !== undefined &&
      fixture.frame.params !== undefined
    ) {
      const schema = paramsSchemas[method];
      assert.ok(schema !== undefined, `${fixture.case}：请求参数没有对应的 v1 schema`);
      assertAccepted(schema, fixture.frame.params, `${fixture.case} 的请求参数`);
      exercisedCommands.add(method);
      continue;
    }
    if (fixture.frame.result !== undefined) {
      const respondsTo = fixture.respondsTo ?? "";
      const schema = resultSchemas[respondsTo];
      assert.ok(
        schema !== undefined,
        `${fixture.case}：应答的 respondsTo=${respondsTo} 没有对应的 result schema`,
      );
      assertAccepted(schema, fixture.frame.result, `${fixture.case} 的应答结果`);
      exercisedCommands.add(respondsTo);
    }
  }
  assert.deepEqual(
    [...exercisedCommands].sort(),
    [...new Set([...Object.keys(paramsSchemas), ...Object.keys(resultSchemas)])].sort(),
    "有 schema 没被夹具覆盖：夹具集与命令集脱节了",
  );
});
