import assert from "node:assert/strict";
import test from "node:test";

import {
  applySubagentTranscriptEvent,
  hydrateSubagentTranscript,
} from "../src/app-subagent-transcript.js";

test("子代理快照：空事件给出默认空 transcript", () => {
  assert.deepEqual(hydrateSubagentTranscript({ sessionId: "s1", events: [] }), {
    sessionId: "s1",
    sequenceNumber: undefined,
    messages: [],
    liveModelText: "",
    toolNamesById: new Map(),
    assistantMessageIdsByToolCallId: new Map(),
  });
});

test("子代理快照：沿用 sessionId 与既有消息", () => {
  const messages = [{ role: "user", content: "hi" }];
  const transcript = hydrateSubagentTranscript({ sessionId: "s1", events: [], messages });
  assert.equal(transcript.sessionId, "s1");
  assert.deepEqual(transcript.messages, messages);
  assert.equal(hydrateSubagentTranscript({ sessionId: "", events: [] }).sessionId, "");
});

test("子代理事件：未知事件被忽略且不改变快照引用", () => {
  const transcript = hydrateSubagentTranscript({ sessionId: "s1", events: [] });
  assert.equal(applySubagentTranscriptEvent(transcript, { type: "unknown.event" }), transcript);
});
