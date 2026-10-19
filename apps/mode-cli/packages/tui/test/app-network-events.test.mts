import assert from "node:assert/strict";
import test from "node:test";

import { applyModelNetworkEvent, applyNetworkRequestEvent } from "../src/app-network-events.js";

interface NetworkRequestLike {
  attempt?: number;
  completedAt?: string;
  error?: string;
  id: string;
  method: string;
  requestId: string;
  source: string;
  startedAt: string;
  status: string;
  updatedAt: string;
  url: string;
}

function applyRequest(payload: Record<string, unknown>, previous: NetworkRequestLike[] = []) {
  let calls = 0;
  let next: NetworkRequestLike[] = previous;
  applyNetworkRequestEvent(
    payload as never,
    ((value: unknown) => {
      calls += 1;
      next =
        typeof value === "function"
          ? (value as (current: NetworkRequestLike[]) => NetworkRequestLike[])(previous)
          : (value as NetworkRequestLike[]);
    }) as never,
  );
  return { calls, next };
}

const COPY = {
  model: {
    retryScheduled: (input: Record<string, unknown>) => `retry:${JSON.stringify(input)}`,
    requestFailed: (message: string) => `failed:${message}`,
    streamStalled: "stalled",
  },
} as never;

function applyModel(payload: Record<string, unknown>) {
  const statuses: string[] = [];
  let calls = 0;
  let next: NetworkRequestLike[] = [];
  applyModelNetworkEvent(
    payload as never,
    ((value: unknown) => {
      calls += 1;
      next =
        typeof value === "function"
          ? (value as (current: NetworkRequestLike[]) => NetworkRequestLike[])([])
          : (value as NetworkRequestLike[]);
    }) as never,
    ((status: string) => statuses.push(status)) as never,
    COPY,
  );
  return { calls, next, statuses };
}

test("网络请求：缺 requestId/method/url 或状态非法时忽略", () => {
  assert.equal(applyRequest({}).calls, 0);
  assert.equal(applyRequest({ requestId: "r", method: "GET", status: "pending" }).calls, 0);
  assert.equal(applyRequest({ requestId: "r", method: "GET", url: "u", status: "weird" }).calls, 0);
});

test("网络请求：合法事件补齐默认 source 并保留 attempt", () => {
  const { calls, next } = applyRequest({
    requestId: "r",
    method: "GET",
    url: "u",
    status: "pending",
    attempt: 2,
  });
  assert.equal(calls, 1);
  assert.equal(next.length, 1);
  assert.equal(next[0]?.id, "r");
  assert.equal(next[0]?.requestId, "r");
  assert.equal(next[0]?.source, "unknown");
  assert.equal(next[0]?.attempt, 2);
  assert.equal(next[0]?.status, "pending");
  assert.equal(next[0]?.completedAt, undefined);
  assert.equal(typeof next[0]?.startedAt, "string");
  assert.equal(next[0]?.startedAt, next[0]?.updatedAt);
});

test("网络请求：同 id 原位更新并移到最前，其它条目保留", () => {
  const { next } = applyRequest({ requestId: "a", method: "GET", url: "u2", status: "complete" }, [
    {
      id: "a",
      method: "GET",
      requestId: "a",
      source: "x",
      startedAt: "s",
      status: "pending",
      updatedAt: "s",
      url: "u",
    },
    {
      id: "b",
      method: "GET",
      requestId: "b",
      source: "x",
      startedAt: "s",
      status: "pending",
      updatedAt: "s",
      url: "u",
    },
  ]);
  assert.equal(next.length, 2);
  assert.equal(next[0]?.url, "u2");
  assert.equal(next[1]?.id, "b");
});

test("网络请求：最多保留 8 条", () => {
  const previous = Array.from({ length: 8 }, (_, index) => ({
    id: `old${index}`,
    method: "GET",
    requestId: `old${index}`,
    source: "x",
    startedAt: "s",
    status: "pending",
    updatedAt: "s",
    url: "u",
  }));
  const { next } = applyRequest(
    { requestId: "new", method: "GET", url: "u", status: "pending" },
    previous,
  );
  assert.equal(next.length, 8);
  assert.equal(next[0]?.id, "new");
  assert.equal(next[7]?.id, "old6");
});

test("模型网络事件：完成态/进行态映射，缺 type 或 requestId 忽略", () => {
  const completed = applyModel({
    type: "model_request_completed",
    requestId: "m1",
    url: "https://x",
  });
  assert.equal(completed.calls, 1);
  assert.equal(completed.next[0]?.status, "complete");
  assert.equal(completed.next[0]?.method, "POST");
  assert.equal(completed.next[0]?.source, "model");
  assert.equal(typeof completed.next[0]?.completedAt, "string");

  const pending = applyModel({ type: "model_request_started", requestId: "m2" });
  assert.equal(pending.next[0]?.status, "pending");
  assert.equal(pending.next[0]?.url, "model");

  assert.equal(applyModel({ type: "model_request_completed" }).calls, 0);
  assert.equal(applyModel({ requestId: "m3" }).calls, 0);
});

test("模型网络事件：失败/停滞/重试写入状态文案", () => {
  const failed = applyModel({ type: "model_request_failed", requestId: "m4", message: "boom" });
  assert.equal(failed.next[0]?.status, "error");
  assert.equal(failed.next[0]?.error, "boom");
  assert.deepEqual(failed.statuses, ["failed:boom"]);

  const stalled = applyModel({ type: "model_stream_stalled", requestId: "m5" });
  assert.deepEqual(stalled.statuses, ["stalled"]);

  const retry = applyModel({
    type: "model_retry_scheduled",
    requestId: "m6",
    attempt: 2,
    maxAttempts: 3,
    delayMs: 1500,
    reason: "rate",
  });
  assert.equal(retry.next[0]?.status, "pending");
  assert.equal(retry.next[0]?.error, "rate");
  assert.deepEqual(retry.statuses, [
    'retry:{"attempt":2,"delay":"1.5s","maxAttempts":3,"reason":"rate"}',
  ]);
});
