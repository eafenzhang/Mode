import assert from "node:assert/strict";
import test from "node:test";
import { botsStateFileSchema, type BotConversationState } from "@mode/shared";

// 出站投递停放状态必须随 bot-state 持久化往返；schema 是严格校验，
// 漏登记字段会在读取时被剥离（表现为补投队列"莫名消失"）。
// 事故背景：docs/specs/bot-outbound-delivery.md（2026-10-10 微信回复丢失）。

test("botsStateFileSchema：对话状态的出站投递字段能往返保真", () => {
  const state = {
    version: 4,
    bots: {
      "bot-1": {
        botId: "bot-1",
        conversations: {
          "private:u1": {
            botId: "bot-1",
            conversationKey: "private:u1",
            conversationKind: "private",
            conversationId: "u1",
            workspacePath: "C:/workspace",
            mode: "draft",
            activeTaskId: null,
            lastContextToken: "tok-abc",
            pendingDeliveryQueue: [
              { text: "上一条回复", providerUserId: "u1", queuedAt: 1000, attempts: 2 },
            ],
            deliveryNoticeAt: 2000,
            updatedAt: 1,
          },
        },
        updatedAt: 1,
      },
    },
  };
  const parsed = botsStateFileSchema.parse(state);
  const conversation = parsed.bots["bot-1"]!.conversations["private:u1"] as BotConversationState;
  assert.equal(conversation.lastContextToken, "tok-abc");
  assert.equal(conversation.deliveryNoticeAt, 2000);
  assert.deepEqual(conversation.pendingDeliveryQueue, [
    { text: "上一条回复", providerUserId: "u1", queuedAt: 1000, attempts: 2 },
  ]);
});

test("botsStateFileSchema：不含新字段的存量对话状态照常解析（向后兼容）", () => {
  const state = {
    version: 4,
    bots: {
      "bot-1": {
        botId: "bot-1",
        conversations: {
          "private:u1": {
            botId: "bot-1",
            conversationKey: "private:u1",
            conversationKind: "private",
            conversationId: "u1",
            workspacePath: "C:/workspace",
            mode: "draft",
            activeTaskId: null,
            updatedAt: 1,
          },
        },
        updatedAt: 1,
      },
    },
  };
  const parsed = botsStateFileSchema.parse(state);
  const conversation = parsed.bots["bot-1"]!.conversations["private:u1"];
  assert.equal(conversation?.lastContextToken, undefined);
  assert.equal(conversation?.pendingDeliveryQueue, undefined);
});

test("classifySendError：仅结构化错误可分类，普通 Error 返回 null", async () => {
  const { BotSendError, classifySendError } = await import("../src/bots/outboundDelivery.js");
  assert.equal(classifySendError(new BotSendError("context_expired", "prepare failed")), "context_expired");
  assert.equal(classifySendError(new BotSendError("retryable", "HTTP 503")), "retryable");
  assert.equal(classifySendError(new BotSendError("session_expired", "HTTP 401")), "session_expired");
  assert.equal(classifySendError(new Error("Weixin iLink /sendmessage failed: ret=1")), null);
  assert.equal(classifySendError("string error"), null);
});

test("planDeliveryRecovery：context_expired 换新 token → 去 token → 停放", async () => {
  const { planDeliveryRecovery } = await import("../src/bots/outboundDelivery.js");
  // 首次失败且有更新 token：换新重试
  assert.deepEqual(
    planDeliveryRecovery("context_expired", { attempt: 0, usedToken: "old", freshToken: "new" }),
    { kind: "retry_with_token", token: "new" },
  );
  // token 无更新：直接去掉 token 重试
  assert.deepEqual(
    planDeliveryRecovery("context_expired", { attempt: 0, usedToken: "old", freshToken: "old" }),
    { kind: "retry_without_token" },
  );
  assert.deepEqual(
    planDeliveryRecovery("context_expired", { attempt: 1, usedToken: "new", freshToken: "new" }),
    { kind: "retry_without_token" },
  );
  // 两步重试用尽：停放
  assert.deepEqual(
    planDeliveryRecovery("context_expired", { attempt: 2, usedToken: undefined, freshToken: undefined }),
    { kind: "park" },
  );
  // 本来就不带 token：无从恢复，直接停放
  assert.deepEqual(planDeliveryRecovery("context_expired", { attempt: 0 }), { kind: "park" });
});

test("planDeliveryRecovery：retryable 退避两次后停放；session_expired 直接停放", async () => {
  const { planDeliveryRecovery, BOT_SEND_RETRY_DELAYS_MS } = await import(
    "../src/bots/outboundDelivery.js"
  );
  assert.deepEqual(planDeliveryRecovery("retryable", { attempt: 0 }), {
    kind: "retry_backoff",
    delayMs: BOT_SEND_RETRY_DELAYS_MS[0],
  });
  assert.deepEqual(planDeliveryRecovery("retryable", { attempt: 1 }), {
    kind: "retry_backoff",
    delayMs: BOT_SEND_RETRY_DELAYS_MS[1],
  });
  assert.deepEqual(planDeliveryRecovery("retryable", { attempt: 2 }), { kind: "park" });
  assert.deepEqual(planDeliveryRecovery("session_expired", { attempt: 0 }), { kind: "park" });
});

test("pendingDeliveryQueue：入队裁剪、队首删除、失败递增与 8 次上限丢弃", async () => {
  const {
    enqueuePendingDelivery,
    removeFirstPendingDelivery,
    bumpFirstPendingDeliveryAttempt,
    BOT_PENDING_DELIVERY_LIMIT,
    BOT_PENDING_DELIVERY_MAX_ATTEMPTS,
  } = await import("../src/bots/outboundDelivery.js");
  let state = enqueuePendingDelivery(undefined, { text: "r1", providerUserId: "u1" }, 1000).queue;
  state = enqueuePendingDelivery(state, { text: "r2", providerUserId: "u1" }, 2000).queue;
  assert.deepEqual(
    state.map((item) => item.text),
    ["r1", "r2"],
  );
  assert.equal(state[0]?.attempts, 0);

  const removed = removeFirstPendingDelivery(state);
  assert.equal(removed.removed?.text, "r1");
  assert.deepEqual(removed.queue.map((item) => item.text), ["r2"]);

  // 队首失败递增 attempts，直至超限丢弃
  let queue = [{ text: "stale", providerUserId: "u1", queuedAt: 1, attempts: 0 }];
  let dropped;
  for (let i = 0; i <= BOT_PENDING_DELIVERY_MAX_ATTEMPTS; i++) {
    const result = bumpFirstPendingDeliveryAttempt(queue);
    queue = result.queue;
    dropped = result.dropped;
  }
  assert.equal(dropped?.text, "stale");
  assert.equal(queue.length, 0);

  // 超上限丢最旧
  let full: ReturnType<typeof enqueuePendingDelivery>["queue"] = [];
  for (let i = 0; i <= BOT_PENDING_DELIVERY_LIMIT; i++) {
    full = enqueuePendingDelivery(full, { text: `m${i}`, providerUserId: "u1" }, i).queue;
  }
  assert.equal(full.length, BOT_PENDING_DELIVERY_LIMIT);
  assert.equal(full[0]?.text, "m1");
});

test("createBotSendQueue：同 bot 严格 FIFO + 失败不阻塞后续；跨 bot 互不阻塞", async () => {
  const { createBotSendQueue, BOT_SEND_MIN_INTERVAL_MS } = await import(
    "../src/bots/outboundDelivery.js"
  );
  const queue = createBotSendQueue(BOT_SEND_MIN_INTERVAL_MS);
  const order: string[] = [];
  const taskA1 = queue.run("botA", async () => {
    await new Promise((resolve) => setTimeout(resolve, 30));
    order.push("a1");
  });
  const taskA2 = queue.run("botA", async () => {
    order.push("a2");
  });
  const taskB1 = queue.run("botB", async () => {
    order.push("b1");
  });
  await Promise.all([taskA1, taskA2, taskB1]);
  // 同 bot 顺序固定；botB 不被 botA 的慢任务阻塞（b1 在 a1 之前完成）
  assert.deepEqual(order, ["b1", "a1", "a2"]);

  // 上一个任务失败不阻塞后续任务，且错误原样抛给调用方
  await assert.rejects(() => queue.run("botC", () => Promise.reject(new Error("boom"))), /boom/);
  const after = await queue.run("botC", () => Promise.resolve("ok"));
  assert.equal(after, "ok");
});

test("createBotSendQueue：同 bot 相邻发送间隔不小于最小间隔", async () => {
  const { createBotSendQueue } = await import("../src/bots/outboundDelivery.js");
  const minIntervalMs = 40;
  const queue = createBotSendQueue(minIntervalMs);
  const starts: number[] = [];
  await Promise.all([
    queue.run("botD", async () => {
      starts.push(Date.now());
    }),
    queue.run("botD", async () => {
      starts.push(Date.now());
    }),
  ]);
  assert.equal(starts.length, 2);
  const gap = Math.max(...starts) - Math.min(...starts);
  assert.ok(gap >= minIntervalMs, `gap ${gap}ms < minInterval ${minIntervalMs}ms`);
});

test("classifyWeixinRequestFailure：HTTP 状态与服务端 errmsg 映射到恢复分类", async () => {
  const { classifyWeixinRequestFailure } = await import(
    "../src/bots/providers/weixinProvider.js"
  );
  assert.equal(classifyWeixinRequestFailure({ httpStatus: 401 }), "session_expired");
  assert.equal(classifyWeixinRequestFailure({ httpStatus: 403 }), "session_expired");
  assert.equal(classifyWeixinRequestFailure({ httpStatus: 429 }), "retryable");
  assert.equal(classifyWeixinRequestFailure({ httpStatus: 500 }), "retryable");
  assert.equal(classifyWeixinRequestFailure({ httpStatus: 503 }), "retryable");
  assert.equal(classifyWeixinRequestFailure({ httpStatus: 404 }), null);
  assert.equal(classifyWeixinRequestFailure({ errmsg: "prepare failed" }), "context_expired");
  assert.equal(classifyWeixinRequestFailure({ errmsg: "some other error" }), null);
  assert.equal(classifyWeixinRequestFailure({}), null);
});
