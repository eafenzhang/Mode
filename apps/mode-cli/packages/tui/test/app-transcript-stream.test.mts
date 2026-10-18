import assert from "node:assert/strict";
import test from "node:test";

import {
  appendStreamingTextDelta,
  appendStreamingThoughtDelta,
  markStreamingMessageComplete,
  markStreamingThoughtComplete,
  projectedTranscriptHasResponse,
} from "../src/app-transcript-stream.js";

const agent = { role: "agent", content: "", streaming: true };

test("文本增量：创建投影消息并把增量写入 text part", () => {
  assert.deepEqual(appendStreamingTextDelta([], "id1", "hi"), [
    {
      content: "",
      id: "id1",
      parts: [{ streamProjected: true, text: "hi", type: "text" }],
      role: "agent",
      streamProjected: true,
      streaming: true,
    },
  ]);
});

test("文本增量：同一 id 累加到同一个 part", () => {
  const once = appendStreamingTextDelta([], "id1", "a");
  const twice = appendStreamingTextDelta(once, "id1", "b");
  assert.equal(twice.length, 1);
  assert.equal(twice[0]?.parts?.[0]?.text, "ab");
});

test("文本增量：空增量直接返回原数组", () => {
  const messages: never[] = [];
  assert.equal(appendStreamingTextDelta(messages as never, "id1", ""), messages);
});

test("消息完成：把 streaming 置为 false", () => {
  assert.deepEqual(markStreamingMessageComplete([agent] as never), [
    { role: "agent", content: "", streaming: false },
  ]);
});

test("思考增量：创建 thinking part 并统计字符数", () => {
  const result = appendStreamingThoughtDelta([], "id1", "think");
  assert.deepEqual(result[0]?.parts?.[0], {
    contentCharCount: 5,
    status: "thinking",
    streamProjected: true,
    text: "think",
    type: "thought",
  });
  const twice = appendStreamingThoughtDelta(result, "id1", "ab");
  assert.equal(twice[0]?.parts?.[0]?.text, "thinkab");
  assert.equal(twice[0]?.parts?.[0]?.contentCharCount, 7);
});

test("思考完成：只把匹配 id 的 thought part 标记为 thought", () => {
  const messages = [
    {
      id: "id1",
      role: "agent",
      content: "",
      streaming: true,
      parts: [{ type: "thought", text: "t", status: "thinking", contentCharCount: 1 }],
    },
  ];
  const done = markStreamingThoughtComplete(messages as never, "id1");
  assert.equal(done[0]?.parts?.[0]?.status, "thought");
  const other = markStreamingThoughtComplete(messages as never, "other");
  assert.equal(other[0]?.parts?.[0]?.status, "thinking");
});

test("响应判定：仅投影消息且（去空白后）完全相等", () => {
  assert.equal(
    projectedTranscriptHasResponse(
      [{ role: "agent", content: "  hi  ", streamProjected: true }] as never,
      "hi",
    ),
    true,
  );
  assert.equal(
    projectedTranscriptHasResponse(
      [{ role: "agent", content: "hello world", streamProjected: true }] as never,
      "hello",
    ),
    false,
  );
  assert.equal(
    projectedTranscriptHasResponse([{ role: "agent", content: "hi" }] as never, "hi"),
    false,
  );
});
