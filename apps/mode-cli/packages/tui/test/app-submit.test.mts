import assert from "node:assert/strict";
import test from "node:test";

import { appendAgentResult } from "../src/app-submit.js";

const EXISTING = [{ role: "user", content: "old" }] as never;

test("提交结果：非空 response 追加为 agent 消息并保留既有消息", () => {
  assert.deepEqual(appendAgentResult(EXISTING, { response: "hi" } as never), [
    { role: "user", content: "old" },
    { content: "hi", role: "agent" },
  ]);
});

test("提交结果：空或空白 response 不追加，但返回新数组", () => {
  const empty = appendAgentResult(EXISTING, { response: "" } as never);
  assert.notEqual(empty, EXISTING);
  assert.deepEqual(empty, EXISTING);
  assert.deepEqual(appendAgentResult(EXISTING, { response: "  " } as never), EXISTING);
});

test("提交结果：restoredMessages 存在时整体替换既有 transcript", () => {
  assert.deepEqual(
    appendAgentResult(EXISTING, {
      response: "hi",
      restoredMessages: [{ role: "user", content: "q" }],
    } as never),
    [
      { role: "user", content: "q" },
      { content: "hi", role: "agent" },
    ],
  );
  assert.deepEqual(appendAgentResult(EXISTING, { response: "hi", restoredMessages: [] } as never), [
    { content: "hi", role: "agent" },
  ]);
});

test("提交结果：恢复的 thought/tool part 补齐展示字段", () => {
  assert.deepEqual(
    appendAgentResult([], {
      response: "",
      restoredMessages: [
        {
          id: "a1",
          role: "agent",
          content: "a",
          parts: [
            { type: "text", text: "a" },
            { type: "thought", text: "hmm" },
          ],
        },
      ],
    } as never),
    [
      {
        id: "a1",
        content: "a",
        role: "agent",
        parts: [
          { text: "a", type: "text" },
          { contentCharCount: 3, status: "thought", text: "hmm", type: "thought" },
        ],
      },
    ],
  );

  assert.deepEqual(
    appendAgentResult(
      [],
      {
        response: "",
        restoredMessages: [
          {
            role: "agent",
            content: "",
            parts: [
              {
                type: "tool",
                toolCallId: "t1",
                toolName: "Read",
                input: { file_path: "/w/a.ts" },
                status: "completed",
                output: "ok",
              },
            ],
          },
        ],
      } as never,
      { workspaceDirectory: "/w" },
    ),
    [
      {
        content: "",
        role: "agent",
        parts: [
          {
            detailLines: [],
            output: "ok",
            status: "completed",
            title: "Read a.ts",
            toolCallId: "t1",
            toolName: "Read",
            type: "tool",
          },
        ],
      },
    ],
  );
});
