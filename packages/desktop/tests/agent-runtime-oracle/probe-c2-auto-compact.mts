// C2 Task 0：自动 compact 的黑盒观察（长历史是否触发自动摘要）。
//
// 用法：pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c2-auto-compact.mts
//   可选环境变量：TURNS（默认 15）、CHARS（每轮助手正文长度，默认 100000）。
//
// 观察方式：每轮结束读 `session/read` 的 `runtime.contextUsage`（messages 字符数）并直接计数
// DB 的 compact_summary / compaction 行；一旦出现就记录触发点。**不猜测阈值**：未出现就写
// `triggered:false` 与当时的实际规模。
import { DatabaseSync } from "node:sqlite";

import { startC2Harness, writeC2Fixture } from "./probe-c2-support.mjs";
import type { JsonObject } from "./session-lifecycle-fixtures.mjs";

const TURNS = Number(process.env.TURNS ?? "15");
const CHARS = Number(process.env.CHARS ?? "100000");
const LONG_TEXT = "L".repeat(CHARS);
const SCRIPT = [() => ({ text: LONG_TEXT })];

function rawCounts(dbPath: string): JsonObject {
  const db = new DatabaseSync(dbPath, { readOnly: true });
  try {
    const messages = db.prepare("select count(*) c from message").get() as { c: number };
    const compactSummaries = db
      .prepare("select count(*) c from message where data like '%compact_summary%'")
      .get() as { c: number };
    const compactionParts = db
      .prepare('select count(*) c from part where data like \'%"type":"compaction"%\'')
      .get() as { c: number };
    const session = db.prepare("select time_compacting from session limit 1").get() as
      | { time_compacting: number | null }
      | undefined;
    return {
      messageRows: messages.c,
      compactSummaryMessages: compactSummaries.c,
      compactionParts: compactionParts.c,
      timeCompacting: session?.time_compacting ?? null,
    };
  } finally {
    db.close();
  }
}

const harness = await startC2Harness({ prefix: "auto-compact", script: SCRIPT });
const turns: JsonObject[] = [];
let triggeredAtTurn: number | null = null;
const baselineEvents = harness.captureState.events.length;
try {
  for (let turn = 1; turn <= TURNS; turn += 1) {
    await harness.sendAndWait(`long turn ${turn}`);
    const read = (await harness.session.request("session/read", {
      sessionId: harness.session.sessionId,
    })) as JsonObject;
    const usage = (read.runtime as JsonObject | undefined)?.contextUsage as JsonObject | undefined;
    const counts = rawCounts(harness.dbPath);
    const turnEvents = harness.captureState.events.slice(baselineEvents);
    const compactionEventTypes = turnEvents
      .filter((event) => JSON.stringify(event.payload ?? "").includes("compaction"))
      .map((event) => event.type);
    turns.push({
      turn,
      contextUsage: usage ?? null,
      messagesChars:
        ((usage?.breakdown as JsonObject[] | undefined) ?? []).find(
          (entry) => entry.source === "messages",
        )?.chars ?? null,
      db: counts,
      eventsSeen: turnEvents.length,
      compactionEventTypes,
    });
    if ((counts.compactSummaryMessages as number) > 0 || (counts.compactionParts as number) > 0) {
      triggeredAtTurn = turn;
      break;
    }
  }
  const dbAfterRun = rawCounts(harness.dbPath);
  const isolation = harness.isolation();
  const fixture = {
    case: "auto-compact-long-history",
    note:
      "C2 Task 0 oracle 捕获：以每轮 100000 字符的脚本化助手正文累加历史，观察自动 compact。" +
      "机器录制结果：triggered=false（未观察到 compact_summary/compaction 行或 compact 事件），" +
      "不要把本夹具反推成「阈值不存在」，只说明该规模/该 provider usage 下未触发。",
    input: { turns: TURNS, charsPerTurn: CHARS, promptPattern: "long turn <n>" },
    expect: {
      isolation,
      turns,
      triggered: triggeredAtTurn !== null,
      triggeredAtTurn,
      totalTurnsObserved: turns.length,
      finalDbCounts: rawCounts(harness.dbPath),
      dbAfterRun,
      eventTypes: [...new Set(harness.captureState.events.map((event) => event.type))],
    },
  };
  const target = await writeC2Fixture("auto-compact-long-history", fixture);
  process.stdout.write(
    `${JSON.stringify(
      {
        fixture: target,
        triggered: triggeredAtTurn !== null,
        turns: turns.length,
        last: turns.at(-1),
        eventTypes: fixture.expect.eventTypes,
      },
      null,
      2,
    )}\n`,
  );
} finally {
  await harness.stop();
}
