// 会话消息读取的 limit 下推语义（docs/specs/remote-workspace-conversation-mirror.md「第五轮修复」）。
//
// 远端对话镜像单次只读最新 200 条消息。若存储层仍把整条会话的消息与 part 全部取出、解码后再切片，
// 单次读取耗时就会随会话历史长度线性增长（长会话上这是秒级到分钟级的差异）。这条测试钉住：
// `messages({ sessionID, limit })` 只取**最新** limit 条，且与全量读的尾部逐项相等（含 parts）。
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import type { CreateSessionInput, MessageId, MessageInfo, MessagePart, SessionId } from "@mode/contracts";
import { createSqliteSessionStore } from "../src/storage/session-store/sqlite-session-store.js";

const SESSION_ID = "ses_messages_limit" as SessionId;
const PROJECT_ID = "prj_messages_limit" as never;

function userMessage(id: string, created: number): MessageInfo {
  return {
    id: id as MessageId,
    sessionID: SESSION_ID,
    role: "user",
    time: { created },
    agent: "mode-agent",
  } as MessageInfo;
}

function textPart(id: string, messageID: string, text: string): MessagePart {
  return {
    id: id as never,
    sessionID: SESSION_ID,
    messageID: messageID as MessageId,
    type: "text",
    text,
  } as MessagePart;
}

/** 建一个真实的 sqlite session store，落 5 条各带 1 个 text part 的消息，然后交给 run。 */
async function withSeededStore(
  run: (store: ReturnType<typeof createSqliteSessionStore>) => Promise<void>,
): Promise<void> {
  const dir = mkdtempSync(join(tmpdir(), "mode-session-messages-limit-"));
  const store = createSqliteSessionStore({ dbPath: join(dir, "session.sqlite") });
  try {
    await store.createSession({
      id: SESSION_ID,
      projectID: PROJECT_ID,
      slug: "messages-limit",
      directory: "/tmp/messages-limit",
      title: "messages limit",
      version: "0.0.0",
    } as CreateSessionInput);
    for (let index = 1; index <= 5; index += 1) {
      const messageId = `m${index}`;
      await store.saveMessage(userMessage(messageId, index * 1000));
      await store.savePart(textPart(`p${index}`, messageId, `内容-${index}`));
    }
    await run(store);
  } finally {
    store.close();
    rmSync(dir, { recursive: true, force: true });
  }
}

function ids(messages: readonly { info: MessageInfo }[]): string[] {
  return messages.map((message) => String(message.info.id));
}

test("limit 只取最新 N 条，且与全量读的尾部逐项相等（含 parts）", async () => {
  await withSeededStore(async (store) => {
    const all = await store.messages({ sessionID: SESSION_ID });
    assert.deepEqual(ids(all), ["m1", "m2", "m3", "m4", "m5"]);

    const lastTwo = await store.messages({ sessionID: SESSION_ID, limit: 2 });
    assert.deepEqual(ids(lastTwo), ["m4", "m5"]);
    // 不只是 id：parts 也必须跟着对上（下推的 part 查询是按「最新 N 条」过滤的）。
    assert.deepEqual(lastTwo, all.slice(-2));
    assert.equal(lastTwo[1]?.parts[0]?.type === "text" ? lastTwo[1].parts[0].text : "", "内容-5");
  });
});

test("limit 大于消息总数时返回全部（不报错、不截断）", async () => {
  await withSeededStore(async (store) => {
    const all = await store.messages({ sessionID: SESSION_ID });
    const oversized = await store.messages({ sessionID: SESSION_ID, limit: 99 });
    assert.deepEqual(ids(oversized), ids(all));
  });
});

test("非法 limit（0 / 负数 / NaN / 非整数）退化成全量读，不把会话读成空", async () => {
  await withSeededStore(async (store) => {
    for (const limit of [0, -3, Number.NaN, 2.7 as number]) {
      const messages = await store.messages({ sessionID: SESSION_ID, limit });
      const expected = limit === 2.7 ? ["m4", "m5"] : ["m1", "m2", "m3", "m4", "m5"];
      assert.deepEqual(ids(messages), expected, `limit=${String(limit)}`);
    }
  });
});
