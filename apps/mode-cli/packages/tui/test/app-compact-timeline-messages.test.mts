import assert from "node:assert/strict";
import test from "node:test";

import {
  failLatestStartedCompactTimeline,
  latestRetryableCompactCommand,
  upsertCompactTimelineMessage,
} from "../src/app-compact-timeline.js";

const timelineMessage = (operationId: string, status: string) => ({
  role: "timeline",
  content: "",
  timeline: { operationId, type: "context_compaction", status },
});

test("compact 时间线：同一 operationId 原位更新，不同 id 追加", () => {
  const started = upsertCompactTimelineMessage([], timelineMessage("o1", "started") as never);
  assert.equal(started.length, 1);
  const completed = upsertCompactTimelineMessage(
    started,
    timelineMessage("o1", "completed") as never,
  );
  assert.equal(completed.length, 1);
  assert.equal(completed[0]?.timeline?.status, "completed");
  const appended = upsertCompactTimelineMessage(
    completed,
    timelineMessage("o2", "started") as never,
  );
  assert.equal(appended.length, 2);
  assert.equal(appended[1]?.timeline?.operationId, "o2");
});

test("compact 时间线：保留非时间线消息与原顺序", () => {
  const messages = [{ role: "user", content: "hi" }, timelineMessage("o1", "started")];
  const updated = upsertCompactTimelineMessage(
    messages as never,
    timelineMessage("o1", "completed") as never,
  );
  assert.deepEqual(updated[0], { role: "user", content: "hi" });
  assert.equal(updated[1]?.timeline?.status, "completed");
});

test("compact 重试：只有 failed 的 compaction 产出 /compact 命令", () => {
  assert.equal(
    latestRetryableCompactCommand([timelineMessage("o1", "failed")] as never),
    "/compact",
  );
  assert.equal(
    latestRetryableCompactCommand([timelineMessage("o1", "completed")] as never),
    undefined,
  );
  assert.equal(
    latestRetryableCompactCommand([timelineMessage("o1", "started")] as never),
    undefined,
  );
});

test("compact 失败收尾：把最近的 started 置为 failed", () => {
  const failed = failLatestStartedCompactTimeline([timelineMessage("o1", "started")] as never);
  assert.equal(failed[0]?.timeline?.status, "failed");
});
