import assert from "node:assert/strict";
import test from "node:test";

import { applySessionEventToState } from "../src/app-events.js";

const COPY = { status: { thinking: "thinking", ready: "ready" } } as never;

interface Call {
  name: string;
  value: unknown;
}

const BASE_VALUES: Record<string, unknown> = {
  activeTurnId: undefined,
  cacheStats: undefined,
  contextUsage: { contextUsed: 0, contextWindow: 0 },
  lastError: undefined,
  liveModelText: "",
  messages: [],
  model: "",
  modifiedFiles: [],
  networkRequests: [],
  queuedInputs: [],
  status: "",
  todos: [],
  usage: undefined,
};

function run(event: Record<string, unknown>): Call[] {
  const calls: Call[] = [];
  const setter =
    (name: string) =>
    (value: unknown): void => {
      const applied =
        typeof value === "function"
          ? (value as (previous: unknown) => unknown)(BASE_VALUES[name])
          : value;
      calls.push({ name, value: applied });
    };
  const handlers = {
    setActiveTurnId: setter("activeTurnId"),
    setCacheStats: setter("cacheStats"),
    setContextUsage: setter("contextUsage"),
    setLastError: setter("lastError"),
    setLiveModelText: setter("liveModelText"),
    setMessages: setter("messages"),
    setModel: setter("model"),
    setThoughtLevel: setter("thoughtLevel"),
    setModifiedFiles: setter("modifiedFiles"),
    setNetworkRequests: setter("networkRequests"),
    setQueuedInputs: setter("queuedInputs"),
    setStatus: setter("status"),
    setTodos: setter("todos"),
    setUsage: setter("usage"),
    setWorkflowMirror: setter("workflowMirror"),
    assistantMessageIdsByToolCallId: new Map(),
    modifiedFileToolCallIds: new Set(),
    toolNamesById: new Map(),
    workspaceDirectory: "/w",
  };
  applySessionEventToState(event as never, handlers as never, COPY);
  return calls;
}

function find(calls: Call[], name: string): Call | undefined {
  return calls.find((call) => call.name === name);
}

test("会话事件：turn_started 设置活动 turn 与 thinking 状态", () => {
  const calls = run({ type: "turn_started", turnId: "t1", payload: {} });
  assert.deepEqual(find(calls, "activeTurnId")?.value, "t1");
  assert.equal(find(calls, "status")?.value, "thinking");
});

test("会话事件：turn_error 记录错误、追加系统消息并清空排队输入", () => {
  const direct = run({ type: "turn_error", payload: { message: "boom" } });
  assert.equal(find(direct, "lastError")?.value, "boom");
  assert.deepEqual(find(direct, "messages")?.value, [{ content: "Error: boom", role: "system" }]);
  assert.deepEqual(find(direct, "queuedInputs")?.value, []);

  const nested = run({ type: "turn_error", payload: { error: { message: "nested" } } });
  assert.equal(find(nested, "lastError")?.value, "nested");
});

test("会话事件：turn_complete 收尾状态、用量、缓存与回退响应", () => {
  const empty = run({ type: "turn_complete", payload: {} });
  assert.equal(find(empty, "status")?.value, "ready");
  assert.ok(empty.some((call) => call.name === "activeTurnId" && call.value === undefined));
  assert.deepEqual(find(empty, "queuedInputs")?.value, []);

  const withUsage = run({
    type: "turn_complete",
    payload: { usage: { inputTokens: 5, outputTokens: 3 } },
  });
  assert.deepEqual(find(withUsage, "usage")?.value, {
    cacheReadTokens: 0,
    cacheWriteTokens: 0,
    inputTokens: 5,
    modelRequestCount: 1,
    outputTokens: 3,
    reasoningTokens: 0,
    source: "provider",
    totalTokens: 8,
    webFetchRequests: 0,
    webSearchRequests: 0,
  });

  const withResponse = run({ type: "turn_complete", payload: { response: "final" } });
  assert.deepEqual(find(withResponse, "messages")?.value, [
    { content: "final", role: "agent", streamProjected: true },
  ]);

  const withCache = run({
    type: "turn_complete",
    payload: { cacheStats: { cachedMessages: 2 } },
  });
  assert.equal(
    (find(withCache, "cacheStats")?.value as { cachedMessages?: number } | undefined)
      ?.cachedMessages,
    2,
  );
});

test("会话事件：未知类型不触发任何 handler", () => {
  assert.deepEqual(run({ type: "unknown_event", payload: {} }), []);
});
