import assert from "node:assert/strict";
import test from "node:test";

// 任务运行中的入站消息队列：上限裁剪、窗口内去重、按序出队。
// 该行为决定“任务运行中发来的消息不丢”这一承诺的正确性。

test("enqueue 追加并返回位次；相同文本在窗口内去重", async () => {
  const { enqueueQueuedMessage } = await import("../src/bots/messageQueue.js");
  const first = enqueueQueuedMessage(undefined, { text: "检查构建", providerUserId: "u1" }, 1_000);
  assert.equal(first.position, 1);
  assert.equal(first.queue.length, 1);

  const duplicate = enqueueQueuedMessage(first.queue, { text: "检查构建" }, 30_000);
  // 去重：位次不变，不追加
  assert.equal(duplicate.position, 1);
  assert.equal(duplicate.queue.length, 1);

  const afterWindow = enqueueQueuedMessage(first.queue, { text: "检查构建" }, 90_000);
  // 超出 60s 去重窗口：视为新消息
  assert.equal(afterWindow.queue.length, 2);
  assert.equal(afterWindow.position, 2);

  const different = enqueueQueuedMessage(first.queue, { text: "再跑一次测试" }, 2_000);
  assert.equal(different.queue.length, 2);
});

test("队列超过上限时丢弃最旧消息", async () => {
  const { enqueueQueuedMessage, BOT_MESSAGE_QUEUE_LIMIT } = await import(
    "../src/bots/messageQueue.js"
  );
  let queue = undefined;
  let dropped;
  for (let index = 0; index < BOT_MESSAGE_QUEUE_LIMIT + 3; index += 1) {
    const result = enqueueQueuedMessage(queue, { text: `msg-${index}` }, index * 10_000);
    queue = result.queue;
    dropped = result.dropped;
  }
  assert.equal(queue.length, BOT_MESSAGE_QUEUE_LIMIT);
  assert.equal(queue[0].text, "msg-3");
  assert.equal(dropped?.text, "msg-2");
});

test("dequeue 按 FIFO 出队；空队列返回 null", async () => {
  const { enqueueQueuedMessage, dequeueQueuedMessage } = await import("../src/bots/messageQueue.js");
  const empty = dequeueQueuedMessage(undefined);
  assert.equal(empty.message, null);
  assert.deepEqual(empty.next, []);

  const { queue } = enqueueQueuedMessage(undefined, { text: "a" }, 1);
  const { queue: queue2 } = enqueueQueuedMessage(queue, { text: "b" }, 2);
  assert.equal(queue2.length, 2);
  const first = dequeueQueuedMessage(queue2);
  assert.equal(first.message?.text, "a");
  const second = dequeueQueuedMessage(first.next);
  assert.equal(second.message?.text, "b");
  assert.equal(dequeueQueuedMessage(second.next).message, null);
});

test("入队时保留发送者快照供重投递", async () => {
  const { enqueueQueuedMessage } = await import("../src/bots/messageQueue.js");
  const { queue } = enqueueQueuedMessage(
    undefined,
    { text: "hi", providerUserId: "u9", displayName: "Zhang", chatId: "c1" },
    1,
  );
  assert.equal(queue[0].providerUserId, "u9");
  assert.equal(queue[0].displayName, "Zhang");
  assert.equal(queue[0].chatId, "c1");
});
