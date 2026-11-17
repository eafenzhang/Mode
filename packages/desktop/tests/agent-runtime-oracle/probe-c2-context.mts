// C2 Task 0：`session/read` 的 `runtime.contextUsage` / `settings.model.available` 观察。
//
// 用法：pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c2-context.mts
//
// 口径：多轮正常 turn 后读快照；同时记录 create/read/resume 三个时刻的字段有无，
// 区分「字段不存在」与「字段为 null」。不手写百分比：观察里没有 percent 字段。
import { startC2Harness, writeC2Fixture } from "./probe-c2-support.mjs";
import { normalizeDeep, type JsonObject } from "./session-lifecycle-fixtures.mjs";

const SEED = { "hello.txt": "hello\n" };
const PROMPTS = ["first prompt", "second prompt", "third prompt"];
const SCRIPT = [{ text: "first answer" }, { text: "second answer" }, { text: "third answer" }];

function snapshotProjection(result: JsonObject): JsonObject {
  const messages = Array.isArray(result.messages) ? (result.messages as JsonObject[]) : [];
  const runtime = result.runtime as JsonObject | undefined;
  return {
    resultKeys: Object.keys(result).sort(),
    runtime: runtime ?? null,
    runtimeKeys: runtime === undefined ? [] : Object.keys(runtime).sort(),
    contextUsage: runtime?.contextUsage ?? null,
    contextUsageKeys:
      runtime?.contextUsage === null || runtime?.contextUsage === undefined
        ? []
        : Object.keys(runtime.contextUsage as JsonObject).sort(),
    projection: result.projection ?? null,
    settings: result.settings ?? null,
    settingsModelKeys: Object.keys((result.settings as JsonObject)?.model ?? {}).sort(),
    messageCount: messages.length,
    messageRoles: messages.map((message) => (message.info as JsonObject)?.role),
    messagePartTypes: messages.map((message) =>
      Array.isArray(message.parts) ? (message.parts as JsonObject[]).map((part) => part.type) : [],
    ),
  };
}

const harness = await startC2Harness({ prefix: "context", script: SCRIPT, seed: SEED });
try {
  const created = (await harness.session.request("session/create", {
    workspace: { workspacePath: harness.cwd, workspaceKey: harness.cwd },
    model: {
      providerId: "oracle-mock",
      modelId: "mock-1",
      options: { reasoningLevel: "disabled" },
    },
    thoughtLevel: "disabled",
    titleGenerationEnabled: false,
  })) as JsonObject;
  // 驱动已经建过一个会话；这里再建一个只用于「create 时刻」的快照观察。
  const normalizedCreate = normalizeDeep(created, harness.context) as JsonObject;
  const createSnapshot = snapshotProjection(normalizedCreate);
  const createSessionId = (created.session as JsonObject).sessionId as string;

  const sendResults: JsonObject[] = [];
  const afterTurns: JsonObject[] = [];
  for (const prompt of PROMPTS) {
    const before = harness.captureState.events.length;
    await harness.sendAndWait(prompt);
    const read = (await harness.session.request("session/read", {
      sessionId: harness.session.sessionId,
    })) as JsonObject;
    const normalizedRead = normalizeDeep(read, harness.context) as JsonObject;
    afterTurns.push({
      prompt,
      events: harness.captureState.events.slice(before).map((event) => event.type),
      snapshot: snapshotProjection(normalizedRead),
    });
    sendResults.push({ prompt, lastEventType: harness.captureState.events.at(-1)?.type });
  }

  const messages = normalizeDeep(
    (await harness.session.request("session/messages", {
      sessionId: harness.session.sessionId,
    })) as JsonObject,
    harness.context,
  ) as JsonObject;
  await harness.session.request("session/close", { sessionId: harness.session.sessionId });
  await harness.session.request("session/close", { sessionId: createSessionId });
  await harness.session.request("session/resume", { sessionId: harness.session.sessionId });
  const afterResume = normalizeDeep(
    (await harness.session.request("session/read", {
      sessionId: harness.session.sessionId,
    })) as JsonObject,
    harness.context,
  ) as JsonObject;
  const list = normalizeDeep(
    (await harness.session.request("session/list", {
      includeArchived: true,
    })) as JsonObject,
    harness.context,
  ) as JsonObject;
  const isolation = harness.isolation();

  const fixture = {
    case: "context-usage-settings",
    note:
      "C2 Task 0 oracle 捕获：session/create 与三轮 session/read 的 runtime.contextUsage、" +
      "projection、settings.model.available；close→resume→read 再观察一次。" +
      "contextUsage 的 breakdown.chars 随工具面/路径变化，比较时应整体替换或忽略具体 chars。",
    input: { seed: SEED, prompts: PROMPTS, script: SCRIPT },
    expect: {
      isolation,
      createSnapshot,
      createSessionId: (normalizedCreate.session as JsonObject).sessionId ?? null,
      sendResults,
      afterTurns,
      afterResume: snapshotProjection(afterResume),
      messages: {
        count: Array.isArray(messages.messages) ? messages.messages.length : null,
        roles: (messages.messages as JsonObject[]).map(
          (message) => (message.info as JsonObject)?.role,
        ),
        partTypes: (messages.messages as JsonObject[]).map((message) =>
          Array.isArray(message.parts)
            ? (message.parts as JsonObject[]).map((part) => part.type)
            : [],
        ),
      },
      list: {
        count: Array.isArray(list.sessions) ? list.sessions.length : null,
        firstKeys:
          Array.isArray(list.sessions) && list.sessions.length > 0
            ? Object.keys(list.sessions[0] as JsonObject).sort()
            : [],
      },
    },
  };
  const target = await writeC2Fixture("context-usage-settings", fixture);
  process.stdout.write(
    `${JSON.stringify(
      {
        fixture: target,
        createContextUsage: createSnapshot.contextUsage,
        createRuntimeKeys: createSnapshot.runtimeKeys,
        afterTurns: afterTurns.map((entry) => ({
          prompt: entry.prompt,
          contextUsageKeys: entry.snapshot.contextUsageKeys,
          contextUsage: entry.snapshot.contextUsage,
          projection: entry.snapshot.projection,
        })),
        afterResume: {
          contextUsage: fixture.expect.afterResume.contextUsage,
          runtimeKeys: fixture.expect.afterResume.runtimeKeys,
        },
        settingsModelKeys: createSnapshot.settingsModelKeys,
      },
      null,
      2,
    )}\n`,
  );
} finally {
  await harness.stop();
}
