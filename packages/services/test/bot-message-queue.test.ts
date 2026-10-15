import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";

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

// 带附件的入队（docs/specs/bot-queued-message-attachments.md）：
// 队列项只记本地路径与展示元数据，且不做文本去重。

const queuedImage = {
  kind: "image" as const,
  filename: "shot.png",
  mimeType: "image/png",
  localPath: "C:/cache/shot.png",
  sizeBytes: 1_024,
};

test("带附件的入队项保留附件；纯文本入队项不带该字段", async () => {
  const { enqueueQueuedMessage } = await import("../src/bots/messageQueue.js");
  const withAttachment = enqueueQueuedMessage(
    undefined,
    { text: "看这张图", providerUserId: "u1", attachments: [queuedImage] },
    1,
  );
  assert.deepEqual(withAttachment.queue[0]!.attachments, [queuedImage]);
  assert.ok(!("dataBase64" in withAttachment.queue[0]!.attachments![0]!));

  const textOnly = enqueueQueuedMessage(undefined, { text: "纯文本" }, 1);
  assert.equal(textOnly.queue[0]!.attachments, undefined);
});

test("带附件的入队不做文本去重（纯附件消息 text 可为空）", async () => {
  const { enqueueQueuedMessage } = await import("../src/bots/messageQueue.js");
  // 两张不同截图、同样空文本：必须各自入队，不能被当成平台重试吞掉第二条。
  const first = enqueueQueuedMessage(undefined, { text: "", attachments: [queuedImage] }, 1_000);
  const second = enqueueQueuedMessage(
    first.queue,
    {
      text: "",
      attachments: [{ ...queuedImage, filename: "second.png", localPath: "C:/cache/second.png" }],
    },
    2_000,
  );
  assert.equal(second.queue.length, 2);
  assert.equal(second.position, 2);
});

test("附件字节合计只统计附件，缺 sizeBytes 按 0 计", async () => {
  const { attachmentBytes, enqueueQueuedMessage, queuedAttachmentBytes } = await import(
    "../src/bots/messageQueue.js"
  );
  const { queue } = enqueueQueuedMessage(
    undefined,
    {
      text: "两张",
      attachments: [queuedImage, { ...queuedImage, filename: "no-size.png", sizeBytes: undefined }],
    },
    1,
  );
  assert.equal(attachmentBytes(queue[0]!.attachments), 1_024);
  assert.equal(queuedAttachmentBytes(queue), 1_024);
  assert.equal(queuedAttachmentBytes(undefined), 0);
});

test("源码契约：排队分支先物化附件、失败才回退，且物化产物不带 base64", async () => {
  const source = await readFile(new URL("../src/bots/botsService.ts", import.meta.url), "utf8");
  assert.ok(
    source.includes("const queuedAttachments = await materializeQueuedAttachments("),
    "排队分支必须先把附件物化到本地再入队（旧行为是'有附件即拒绝'）",
  );
  assert.ok(
    !/if \(message\.attachments && message\.attachments\.length > 0\)/.test(source),
    "旧的'有附件即拒绝'分支必须已被替换",
  );
  assert.ok(
    source.includes("queuedAttachmentBytes(auth.context.queuedMessages)"),
    "入队前必须做队列附件预算判定",
  );
  const start = source.indexOf("async function materializeQueuedAttachments");
  assert.ok(start >= 0, "缺少 materializeQueuedAttachments");
  const body = source.slice(start, start + 1_800);
  assert.ok(
    !body.includes("dataBase64"),
    "物化产物只能记 localPath：base64 写进 bot-state 会把状态文件撑到 MB 级",
  );
});
