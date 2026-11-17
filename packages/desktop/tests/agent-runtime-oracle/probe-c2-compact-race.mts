// C2 Task 0：`session/compact` 的并发窗口观察（already_running / skipped / 运行中 turn 的拒绝）。
//
// 用法：pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c2-compact-race.mts
//
// 同 tick 连发两个 compact 在 oracle 里观察到两种真实结果（重复运行会切换）：
//   A. 第二次 `{state:"already_running"}`，只有第一次产生摘要/事件；
//   B. 两次都 `{state:"accepted"}`，第二次在第一次完成后以 `status:"skipped"` 结束。
// 因此本探针跑多轮 trial 并把每轮结果都机器录制进同一夹具，不把任一种当成唯一契约。
import { normalizeDeep, type JsonObject } from "./session-lifecycle-fixtures.mjs";
import {
  delay,
  readC2Db,
  readSessionColumn,
  startC2Harness,
  stripErrorStack,
  waitForTerminals,
  writeC2Fixture,
} from "./probe-c2-support.mjs";

const SEED = { "hello.txt": "hello\n" };
const SCRIPT = [
  { text: "first turn answer" },
  { text: "second turn answer" },
  { text: "later turn answer" },
];
const TRIALS = Number(process.env.TRIALS ?? "4");

function summarizeResult(entry: PromiseSettledResult<unknown>): JsonObject {
  if (entry.status === "rejected") {
    return { status: "rejected", message: String(entry.reason) };
  }
  const value = entry.value as JsonObject;
  const compact = value.compact as JsonObject | undefined;
  return {
    status: "fulfilled",
    response: value.response,
    compact: compact ?? null,
    compactKeys: compact === undefined ? [] : Object.keys(compact),
    snapshotRuntime: (value.snapshot as JsonObject | undefined)?.runtime,
    snapshotProjection: (value.snapshot as JsonObject | undefined)?.projection,
  };
}

function compactionCounts(db: JsonObject): JsonObject {
  const parts = ((db.part as JsonObject).rows as JsonObject[]) ?? [];
  const messages = ((db.message as JsonObject).rows as JsonObject[]) ?? [];
  return {
    compactionParts: parts.filter((row) => (row.data as JsonObject)?.type === "compaction").length,
    compactSummaryMessages: messages.filter((row) => {
      const data = row.data as JsonObject;
      return (data.semantics as JsonObject | undefined)?.kind === "compact_summary";
    }).length,
    skippedTimelineMessages: messages.filter(
      (row) => (row.data as JsonObject)?.finish === "skipped",
    ).length,
  };
}

/** 并发 compact 事件里只留状态机字段（模型请求推送会把夹具撑大且与手动 compact 重复）。 */
function compactStatusEvents(events: readonly JsonObject[]): JsonObject[] {
  return events
    .filter((event) => {
      if (event.type === "turn.started" || event.type === "turn.completed") return true;
      if (event.type !== "session.updated") return false;
      const payload = event.payload as JsonObject;
      return typeof payload.status === "string" && typeof payload.operationId === "string";
    })
    .map((event) => ({ type: event.type, payload: stripErrorStack(event.payload) }));
}

async function sameTickTrial(trial: number): Promise<JsonObject> {
  const harness = await startC2Harness({
    prefix: `compact-race-${trial}`,
    script: SCRIPT,
    seed: SEED,
  });
  try {
    await harness.sendAndWait("first prompt");
    await harness.sendAndWait("second prompt");
    const boundary = harness.captureState.events.length;
    let sampleCount = 0;
    const sampleValues = new Set<string>();
    let settled = false;
    const compact1 = harness.session.request("session/compact", {
      sessionId: harness.session.sessionId,
      inputId: `input_compact_trial_${trial}_1`,
    });
    const compact2 = harness.session.request("session/compact", {
      sessionId: harness.session.sessionId,
      inputId: `input_compact_trial_${trial}_2`,
    });
    const poll = (async () => {
      while (!settled) {
        sampleCount += 1;
        sampleValues.add(String(readSessionColumn(harness.dbPath, "time_compacting") ?? "null"));
        await delay(1);
      }
    })();
    const settledResults = await Promise.allSettled([compact1, compact2]);
    settled = true;
    await poll;
    await delay(500);
    const isolation = harness.isolation();
    const db = readC2Db(harness.dbPath, harness.context);
    const events = harness.captureState.events.slice(boundary);
    return {
      trial,
      isolation,
      results: settledResults
        .map((entry) =>
          entry.status === "fulfilled"
            ? { status: "fulfilled", value: normalizeDeep(entry.value, harness.context) }
            : entry,
        )
        .map(summarizeResult),
      requests: harness.captureState.requests.filter(
        (request) => request.method === "session/compact",
      ),
      statusEvents: compactStatusEvents(events),
      eventTypes: events.map((event) => event.type),
      timeCompactingSamples: {
        sampled: sampleCount > 0,
        distinctValues: [...sampleValues].sort(),
      },
      timeCompactingNonEmptySamples: [...sampleValues].filter((value) => value !== "null").length,
      dbCounts: compactionCounts(db),
      modelRequestCount: harness.rawBodies.length,
    };
  } finally {
    await harness.stop();
  }
}

async function duringRunningTurn(): Promise<JsonObject> {
  const harness = await startC2Harness({ prefix: "compact-running", script: SCRIPT, seed: SEED });
  try {
    const boundary = harness.captureState.events.length;
    await harness.session.send("turn that is still running");
    let rejected: string | undefined;
    try {
      await harness.session.request("session/compact", {
        sessionId: harness.session.sessionId,
        inputId: "input_compact_running_1",
      });
    } catch (error) {
      rejected = String(error);
    }
    const dbDuring = readC2Db(harness.dbPath, harness.context);
    await waitForTerminals(harness.captureState, 1);
    const isolation = harness.isolation();
    return {
      isolation,
      rejected,
      requests: harness.captureState.requests.filter(
        (request) => request.method === "session/compact",
      ),
      events: harness.captureState.events
        .slice(boundary)
        .map((event) => ({ ...event, payload: stripErrorStack(event.payload) })),
      eventTypes: harness.captureState.events.slice(boundary).map((event) => event.type),
      dbDuring,
      dbCountsDuring: compactionCounts(dbDuring),
    };
  } finally {
    await harness.stop();
  }
}

const trials: JsonObject[] = [];
for (let trial = 1; trial <= TRIALS; trial += 1) {
  trials.push(await sameTickTrial(trial));
}
const running = await duringRunningTurn();
const outcomes = trials.map((trial) =>
  ((trial.results as JsonObject[]) ?? [])
    .map((result) => (result.compact as JsonObject | undefined)?.state ?? result.status)
    .join("+"),
);
const targets = [
  await writeC2Fixture("compact-same-tick", {
    case: "compact-same-tick",
    note:
      "C2 Task 0 oracle 捕获：同一 tick 连发两个 session/compact，多轮 trial。机器观察到两种形态：" +
      "第二次 already_running（只有第一次执行），或两次 accepted 后第二次以 status=skipped 结束" +
      "（turn.completed response='Context is up to date; no compression needed'）。",
    input: {
      seed: SEED,
      prompts: ["first prompt", "second prompt"],
      script: SCRIPT,
      trials: TRIALS,
    },
    expect: {
      trials,
      outcomes,
      outcomeKinds: [...new Set(outcomes)],
      timeCompactingNonEmptySamplesTotal: trials.reduce(
        (sum, trial) => sum + Number(trial.timeCompactingNonEmptySamples ?? 0),
        0,
      ),
    },
  }),
  await writeC2Fixture("compact-while-turn-running", {
    case: "compact-while-turn-running",
    note:
      "C2 Task 0 oracle 捕获：session/send 受理后立刻 session/compact，记录运行时拒绝形状" +
      "（-32010 Cannot compact while a prompt is running，不是 already_running）。",
    input: { seed: SEED, prompts: ["turn that is still running"], script: SCRIPT },
    expect: running,
  }),
];
process.stdout.write(
  `${JSON.stringify(
    {
      fixtures: targets,
      outcomes,
      outcomeKinds: [...new Set(outcomes)],
      perTrial: trials.map((trial) => ({
        trial: trial.trial,
        results: (trial.results as JsonObject[]).map((result) => result.compact ?? result),
        dbCounts: trial.dbCounts,
        eventTypes: trial.eventTypes,
        timeCompactingSamples: trial.timeCompactingSamples,
        timeCompactingNonEmptySamples: trial.timeCompactingNonEmptySamples,
      })),
      running: { rejected: running.rejected, dbCountsDuring: running.dbCountsDuring },
    },
    null,
    2,
  )}\n`,
);
