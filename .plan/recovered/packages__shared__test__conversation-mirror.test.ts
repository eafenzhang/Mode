/**
 * 远程工作区对话记录镜像的投影层回归（docs/specs/remote-workspace-conversation-mirror.md）。
 * 运行：npx tsx --test packages/shared/test/conversation-mirror.test.ts
 *
 * 钉住四件容易回归的事：
 * 1) 只镜像本机本地 scope 会话，且镜像副本（mirror- 前缀）不再被转发；
 * 2) 目标会话 id 由来源会话 id 确定性派生（重同步幂等，重连换端口也不重复）；
 * 3) 有界化：消息条数 / 单条文本长度 / 单记录字节预算，超限置 truncated；
 * 4) 导入 payload 与「是否需要导入」判据。
 */
import assert from "node:assert/strict";
import test from "node:test";
import type { ModeMessageWithParts } from "../src/mode-protocol-legacy-types.js";

function textPart(messageId: string, text: string) {
  return { partId: `${messageId}-p`, sessionId: "s1", messageId, type: "text" as const, text };
}

function message(
  messageId: string,
  role: "user" | "assistant",
  texts: string[],
  options?: { hidden?: boolean; created?: number },
): ModeMessageWithParts {
  return {
    info: {
      messageId,
      sessionId: "s1",
      role,
      time: { created: options?.created ?? 1_000 },
      agent: "mode-agent",
      ...(options?.hidden
        ? {
            semantics: {
              origin: "import",
              kind: "shared_context",
              uiVisibility: "hidden",
              providerVisibility: "visible",
              transcriptVisibility: "visible",
            },
          }
        : {}),
    },
    parts: texts.map((text, index) => textPart(`${messageId}-${index}`, text)),
  } as unknown as ModeMessageWithParts;
}

test("确定性目标 id：同来源同 id，不同来源不同 id，带 mirror- 前缀", async () => {
  const { buildMirrorImportSessionId, isMirrorSessionId } = await import(
    "../src/conversation-mirror.js"
  );

  const first = buildMirrorImportSessionId("ses-abc");
  assert.equal(first, buildMirrorImportSessionId("ses-abc"));
  assert.notEqual(first, buildMirrorImportSessionId("ses-abd"));
  assert.equal(isMirrorSessionId(first), true);
  assert.equal(isMirrorSessionId("ses-abc"), false);
});

test("只选本机本地 scope 会话：带 identity 的行与镜像副本都不参与镜像", async () => {
  const { selectMirrorSourceTasks } = await import("../src/conversation-mirror.js");

  const selected = selectMirrorSourceTasks(
    [
      { taskId: "local-old", title: "old", createdAt: 1, updatedAt: 10 },
      {
        taskId: "remote",
        title: "remote",
        createdAt: 1,
        updatedAt: 99,
        workspaceIdentity: "remote:lan:h:1:D:/p",
      },
      { taskId: "mirror-ses-x", title: "副本", createdAt: 1, updatedAt: 98 },
      {
        taskId: "blank-identity",
        title: "blank",
        createdAt: 1,
        updatedAt: 60,
        workspaceIdentity: "  ",
      },
    ],
    5,
  );

  assert.deepEqual(
    selected.map((task) => task.taskId),
    ["blank-identity", "local-old"],
  );
});

test("消息提取：跳过隐藏 synthetic、空文本与非 text part", async () => {
  const { extractConversationMirrorMessages } = await import("../src/conversation-mirror.js");

  const extracted = extractConversationMirrorMessages([
    message("m1", "user", ["帮我看看这个报错"]),
    message("m2", "assistant", ["先看日志", "再看构建产物"]),
    message("m3", "user", ["隐藏的分享上下文"], { hidden: true }),
    message("m4", "assistant", []),
  ]);

  assert.deepEqual(
    extracted.map((item) => [item.role, item.text]),
    [
      ["user", "帮我看看这个报错"],
      ["assistant", "先看日志\n\n再看构建产物"],
    ],
  );
  assert.equal(extracted[0]?.timestamp, 1_000);
});

test("没有可读文本时不产生记录（不导入空会话）", async () => {
  const { buildConversationMirrorRecord } = await import("../src/conversation-mirror.js");
  const task = { taskId: "s1", title: "标题", createdAt: 1, updatedAt: 2 };

  assert.equal(buildConversationMirrorRecord({ task, messages: [] }), null);
  assert.equal(
    buildConversationMirrorRecord({ task, messages: [message("m1", "user", [], { hidden: true })] }),
    null,
  );
});

test("有界化：消息条数、单条文本长度、单记录字节预算", async () => {
  const { CONVERSATION_MIRROR_LIMITS, boundConversationMirrorRecord } = await import(
    "../src/conversation-mirror.js"
  );

  const manyMessages = Array.from(
    { length: CONVERSATION_MIRROR_LIMITS.maxMessagesPerSession + 30 },
    (_unused, index) => ({
      role: "user" as const,
      text:
        index === CONVERSATION_MIRROR_LIMITS.maxMessagesPerSession + 29
          ? "字".repeat(CONVERSATION_MIRROR_LIMITS.maxTextCharsPerMessage + 500)
          : `m${index}`,
    }),
  );

  const bounded = boundConversationMirrorRecord({
    sessionId: "s-many",
    title: "很多消息",
    createdAt: 1,
    updatedAt: 2,
    messageCount: manyMessages.length,
    truncated: false,
    messages: manyMessages,
  });

  assert.equal(bounded.messages.length, CONVERSATION_MIRROR_LIMITS.maxMessagesPerSession);
  assert.equal(bounded.truncated, true);
  assert.equal(
    bounded.messages.at(-1)?.text.length,
    CONVERSATION_MIRROR_LIMITS.maxTextCharsPerMessage + 1,
  );

  // 字节预算：单条记录超限时从最旧消息开始丢，直到落进上限。
  const hugeMessages = Array.from(
    { length: CONVERSATION_MIRROR_LIMITS.maxMessagesPerSession },
    () => ({
      role: "assistant" as const,
      text: "海".repeat(CONVERSATION_MIRROR_LIMITS.maxTextCharsPerMessage),
    }),
  );
  const byteBounded = boundConversationMirrorRecord({
    sessionId: "s-huge",
    title: "超大",
    createdAt: 1,
    updatedAt: 2,
    messageCount: hugeMessages.length,
    truncated: false,
    messages: hugeMessages,
  });
  assert.ok(byteBounded.messages.length < hugeMessages.length);
  assert.equal(byteBounded.truncated, true);
});

test("导入 payload：source / 时间 / 来源 id / 消息角色与文本", async () => {
  const {
    CONVERSATION_MIRROR_IMPORT_SOURCE,
    buildConversationMirrorImportedHistory,
    buildConversationMirrorRecord,
  } = await import("../src/conversation-mirror.js");

  const record = buildConversationMirrorRecord({
    task: { taskId: "ses-1", title: "标题", createdAt: 10, updatedAt: 20 },
    messages: [message("m1", "user", ["你好"], { created: 11 })],
  });
  assert.ok(record);

  const history = buildConversationMirrorImportedHistory(record);
  assert.equal(history.source, CONVERSATION_MIRROR_IMPORT_SOURCE);
  assert.equal(history.title, "标题");
  assert.equal(history.createdAt, 10);
  assert.equal(history.updatedAt, 20);
  assert.equal(history.originSessionId, "ses-1");
  assert.deepEqual(history.messages, [{ role: "user", content: "你好", timestamp: 11 }]);
});

test("是否需要导入：目标缺失或更旧才导入（用户续聊后目标更新则不重导）", async () => {
  const { shouldImportMirrorRecord } = await import("../src/conversation-mirror.js");
  const record = {
    sessionId: "ses-1",
    title: "t",
    createdAt: 1,
    updatedAt: 100,
    messageCount: 1,
    truncated: false,
    messages: [{ role: "user" as const, text: "x" }],
  };

  assert.equal(shouldImportMirrorRecord({ record, targetUpdatedAt: undefined }), true);
  assert.equal(shouldImportMirrorRecord({ record, targetUpdatedAt: 99 }), true);
  assert.equal(shouldImportMirrorRecord({ record, targetUpdatedAt: 100 }), false);
  assert.equal(shouldImportMirrorRecord({ record, targetUpdatedAt: 200 }), false);
});
