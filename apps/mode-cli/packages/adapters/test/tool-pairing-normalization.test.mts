// tool_use / tool_result 配对自愈（docs/specs/provider-history-tool-pairing.md）的验收测试。
// 落点与运行前提对齐 core 的既有惯例（cua-display-golden.test.mts 头注）：
//   - 源码从 ../dist 导入，先 build：pnpm --filter @mode/adapters build；
//   - 语言政策：测试文件用 .mts；
//   - mode-cli 包测试不进 release.yml verify，本文件先落地本地脚本（adapters package.json test）。
import assert from "node:assert/strict";
import test from "node:test";

import type { ModelInputMessage } from "@mode/contracts";
import {
  isToolResultPairingRejection,
  repairOrphanToolResults,
  repairToolResultPairingAfterInvalidRequest,
} from "../dist/model/tool-pairing-normalization.js";

const PAIRING_ERROR_MESSAGE =
  "unexpected `messages.823.content.0: tool_use_id` found in `tool_result` blocks: " +
  "call_00_gwRWMn1UQ4eeJpdSNxc79094. Each `tool_result` block must have a " +
  "corresponding `tool_use` block in the previous message.";

function pairingError(statusCode = 400): Error {
  return Object.assign(new Error(PAIRING_ERROR_MESSAGE), { statusCode });
}

function assistant(text: string, toolCallIds: readonly string[] = []): ModelInputMessage {
  return {
    role: "assistant",
    content: text,
    ...(toolCallIds.length > 0
      ? {
          toolCalls: toolCallIds.map((id) => ({ id, name: "Read", input: {} })),
        }
      : {}),
  };
}

function toolResult(id: string, content = "ok"): ModelInputMessage {
  return { role: "tool", content, toolCallId: id, toolName: "Read" };
}

function user(text: string): ModelInputMessage {
  return { role: "user", content: text };
}

function system(text: string): ModelInputMessage {
  return { role: "system", content: text };
}

test("配对合法的历史原样保留（相邻 user 穿插 + 多结果共享 assistant）", () => {
  const messages: ModelInputMessage[] = [
    user("hello"),
    assistant("on it", ["call_a", "call_b"]),
    toolResult("call_a"),
    toolResult("call_b"),
    assistant("done"),
  ];
  const repaired = repairOrphanToolResults(messages);
  assert.equal(repaired, undefined);
  assert.equal(repairToolResultPairingAfterInvalidRequest(messages, pairingError()), undefined);
});

test("相邻 user 夹在 assistant 与 tool 结果之间仍算配对（wire 会合并分组）", () => {
  const messages: ModelInputMessage[] = [
    assistant("call", ["call_a"]),
    user("<system-reminder>…</system-reminder>"),
    toolResult("call_a"),
  ];
  assert.equal(repairOrphanToolResults(messages), undefined);
});

test("声明方 assistant 缺失：孤儿 tool_result 被丢弃并上报 id", () => {
  const messages: ModelInputMessage[] = [
    user("go"),
    assistant("text only"),
    toolResult("call_00_orphan"),
    assistant("after"),
  ];
  const repaired = repairOrphanToolResults(messages);
  assert.ok(repaired);
  assert.deepEqual(repaired!.droppedToolCallIds, ["call_00_orphan"]);
  assert.deepEqual(repaired!.reanchoredToolCallIds, []);
  assert.deepEqual(
    repaired!.messages.map((message) => message.role),
    ["user", "assistant", "assistant"],
  );
});

test("history 以 tool 结果开头：无任何声明方 → 丢弃", () => {
  const messages: ModelInputMessage[] = [toolResult("call_head"), assistant("hi")];
  const repaired = repairOrphanToolResults(messages);
  assert.ok(repaired);
  assert.deepEqual(repaired!.droppedToolCallIds, ["call_head"]);
  assert.deepEqual(
    repaired!.messages.map((message) => message.role),
    ["assistant"],
  );
});

test("system 隔断在声明方与结果之间：结果重新锚定到声明方紧后", () => {
  const messages: ModelInputMessage[] = [
    assistant("call", ["call_a"]),
    system("mid-conversation system"),
    toolResult("call_a"),
    assistant("after"),
  ];
  const repaired = repairOrphanToolResults(messages);
  assert.ok(repaired);
  assert.deepEqual(repaired!.reanchoredToolCallIds, ["call_a"]);
  assert.deepEqual(repaired!.droppedToolCallIds, []);
  assert.deepEqual(repaired!.messages, [
    assistant("call", ["call_a"]),
    toolResult("call_a"),
    system("mid-conversation system"),
    assistant("after"),
  ]);
});

test("同一 id 的第二个 tool_result 视为重复：丢弃后来者，不重复锚定", () => {
  const messages: ModelInputMessage[] = [
    assistant("call", ["call_a"]),
    toolResult("call_a"),
    system("隔断"),
    toolResult("call_a", "dup"),
  ];
  const repaired = repairOrphanToolResults(messages);
  assert.ok(repaired);
  assert.deepEqual(repaired!.droppedToolCallIds, ["call_a"]);
  assert.deepEqual(repaired!.reanchoredToolCallIds, []);
  assert.equal(repaired!.messages.filter((message) => message.role === "tool").length, 1);
});

test("混合场景：合法对保留、孤儿丢弃、隔断对重锚，顺序按原始次序", () => {
  const messages: ModelInputMessage[] = [
    assistant("round1", ["call_ok"]),
    toolResult("call_ok"),
    assistant("round2", ["call_split"]),
    system("boundary"),
    toolResult("call_split"),
    assistant("round3"),
    toolResult("call_lost"),
  ];
  const repaired = repairOrphanToolResults(messages);
  assert.ok(repaired);
  assert.deepEqual(repaired!.reanchoredToolCallIds, ["call_split"]);
  assert.deepEqual(repaired!.droppedToolCallIds, ["call_lost"]);
  assert.deepEqual(repaired!.messages, [
    assistant("round1", ["call_ok"]),
    toolResult("call_ok"),
    assistant("round2", ["call_split"]),
    toolResult("call_split"),
    system("boundary"),
    assistant("round3"),
  ]);
});

test("错误匹配：400 + Anthropic 配对文案才触发；其余返回 undefined", () => {
  assert.equal(isToolResultPairingRejection(pairingError()), true);
  assert.equal(isToolResultPairingRejection(pairingError(429)), false);
  assert.equal(
    isToolResultPairingRejection(
      Object.assign(new Error("signature in thinking block is invalid"), { statusCode: 400 }),
    ),
    false,
  );
  assert.equal(isToolResultPairingRejection(new Error(PAIRING_ERROR_MESSAGE)), false);
  assert.equal(isToolResultPairingRejection(undefined), false);
});

test("文案匹配但没有孤儿：不产生修复副本（no-op 不消耗重试）", () => {
  const messages: ModelInputMessage[] = [assistant("call", ["call_a"]), toolResult("call_a")];
  assert.equal(repairToolResultPairingAfterInvalidRequest(messages, pairingError()), undefined);
});

test("文案匹配且有孤儿：返回修复副本与取证 id 列表", () => {
  const messages: ModelInputMessage[] = [
    assistant("text only"),
    toolResult("call_00_gwRWMn1UQ4eeJpdSNxc79094"),
  ];
  const repaired = repairToolResultPairingAfterInvalidRequest(messages, pairingError());
  assert.ok(repaired);
  assert.deepEqual(repaired!.droppedToolCallIds, ["call_00_gwRWMn1UQ4eeJpdSNxc79094"]);
  assert.deepEqual(repaired!.messages, [assistant("text only")]);
});
