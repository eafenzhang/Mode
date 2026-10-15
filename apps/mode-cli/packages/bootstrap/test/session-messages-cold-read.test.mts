/**
 * 冷会话读取：会话落在 store 里但不在活跃 runtime 时，`session/messages` 必须还能读出来。
 *
 * 这是远程工作区对话镜像的前提——镜像读的是对端**冷**会话，走不了 `requireSession`。
 * 修复前它每个冷会话都抛 `Session is not active`，于是整轮导入恒为 0 且对外报 ready
 * （docs/specs/remote-workspace-conversation-mirror.md 不变量 7 / 「2026-10-15 冷读修复」）。
 *
 * 另一件必须在同一用例里钉住的事：**返回值要走 wire 合同**。store 行是内部形状
 * （`info.id` / `part.messageID`），协议是 `info.messageId`；原样返回时调用方
 * `resolveResponse` 逐条校验会报 `info.messageId: expected string, received undefined`，
 * 于是「有消息的会话」永远读不出来（只有空会话碰巧通过校验）——线上实测就是这条
 * （`mode-session.readSessionMessages FAIL` + ZodError，镜像恒导入 0）。
 *
 * 运行：pnpm --filter @mode/bootstrap test
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { modeSessionMessagesResultSchema } from "@mode/shared";
import { readMessages } from "../src/mode-protocol/server-operations.js";
import type { ModeProtocolAgentServerContext } from "../src/mode-protocol/server-types.js";

const SESSION_ID = "sess-cold-1";

function persistedMessage(id: string, text: string): unknown {
  return {
    info: { id, sessionID: SESSION_ID, role: "user", time: { created: 1 }, agent: "mode-agent" },
    parts: [{ id: `${id}-p`, sessionID: SESSION_ID, messageID: id, type: "text", text }],
  };
}

function fakeStore(session: unknown, messages: unknown[], calls: string[] = []) {
  return {
    async getSession() {
      calls.push("getSession");
      return session;
    },
    async messages() {
      calls.push("messages");
      return messages;
    },
  };
}

function makeContext(sessionStore: unknown) {
  const sessions = new Map<string, unknown>();
  return {
    sessions,
    context: {
      sessions,
      deps: { sessionStore },
    } as unknown as ModeProtocolAgentServerContext,
  };
}

test("冷会话（不在活跃 runtime）直接读 store，不再抛 Session is not active", async () => {
  const calls: string[] = [];
  const { context, sessions } = makeContext(
    fakeStore({ id: SESSION_ID }, [persistedMessage("m1", "冷会话内容")], calls),
  );
  assert.equal(sessions.size, 0, "夹具前提：该会话不在活跃 runtime 里");

  const result = await readMessages(context, { sessionId: SESSION_ID });

  assert.equal(result.messages.length, 1);
  assert.equal(result.messages[0]?.parts[0]?.text, "冷会话内容");
  assert.deepEqual(calls, ["getSession", "messages"]);
  // 关键不变量：不能为了读消息把会话恢复进 runtime（不变量 7「不启动额外 runtime」）。
  assert.equal(sessions.size, 0);
});

test("返回的是 wire 形状（info.messageId），并通过协议校验", async () => {
  const { context } = makeContext(fakeStore({ id: SESSION_ID }, [persistedMessage("m1", "内容")]));

  const result = await readMessages(context, { sessionId: SESSION_ID });

  // store 行只有 info.id；直接回给调用方会被 resolveResponse 判成
  // `info.messageId: undefined`，有消息的会话一律读失败。
  assert.equal(result.messages[0]?.info.messageId, "m1");
  assert.equal(result.messages[0]?.info.sessionId, SESSION_ID);
  assert.equal(result.messages[0]?.parts[0]?.partId, "m1-p");
  assert.equal(result.messages[0]?.parts[0]?.messageId, "m1");
  const parsed = modeSessionMessagesResultSchema.safeParse(result);
  assert.equal(parsed.success, true, parsed.success ? "" : JSON.stringify(parsed.error.issues));
});

test("空会话也走同一条路径，仍通过协议校验", async () => {
  const { context } = makeContext(fakeStore({ id: SESSION_ID }, []));

  const result = await readMessages(context, { sessionId: SESSION_ID });

  assert.deepEqual(result.messages, []);
  assert.equal(modeSessionMessagesResultSchema.safeParse(result).success, true);
});

test("冷读沿用 limit / afterMessageId 切片语义", async () => {
  const { context } = makeContext(
    fakeStore({ id: SESSION_ID }, [
      persistedMessage("m1", "a"),
      persistedMessage("m2", "b"),
      persistedMessage("m3", "c"),
    ]),
  );

  const limited = await readMessages(context, { sessionId: SESSION_ID, limit: 2 });
  assert.deepEqual(
    limited.messages.map((message) => message.info.messageId),
    ["m2", "m3"],
  );

  // afterMessageId 是按 store 行 id 匹配的（切片在映射之前完成），出参才是 wire 形状。
  const after = await readMessages(context, { sessionId: SESSION_ID, afterMessageId: "m1" });
  assert.deepEqual(
    after.messages.map((message) => message.info.messageId),
    ["m2", "m3"],
  );
});

test("session 根本不存在时保持 sessionUnavailable，不伪装成空会话", async () => {
  const { context } = makeContext(fakeStore(null, []));

  await assert.rejects(
    () => readMessages(context, { sessionId: "sess-missing" }),
    (error: Error) => {
      assert.match(error.message, /Session is not active: sess-missing/);
      return true;
    },
  );
});

test("没有 session store 时同样失败，不静默返回空列表", async () => {
  const { context } = makeContext(undefined);

  await assert.rejects(
    () => readMessages(context, { sessionId: SESSION_ID }),
    (error: Error) => {
      assert.match(error.message, /Session is not active/);
      return true;
    },
  );
});
