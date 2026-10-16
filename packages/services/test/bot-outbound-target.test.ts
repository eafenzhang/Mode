import assert from "node:assert/strict";
import test from "node:test";

// 出站目标统一（docs/specs/bot-outbound-target.md）：
//   面向会话的文本/卡片/媒体目标 = 所属会话 = actor.chatId ?? actor.providerUserId
//   （群回群 chatId、私聊回对方 userId），对齐 MyAgents `body.chatid || body.from.userid`。
// 事故根因：流式卡片与桌面输入镜像两条路径各自手写了目标表达式，漂移成
// 「发送者/绑定用户」，企微群回合的正文因此进了私聊、群里只剩占位流。
// 这里用源码契约锁住三条不变量（服务层闭包内无法直接单测，沿用
// bot-group-chat / weixin-provider-self-id 的源码契约模式）。

const botsServiceSource = await import("node:fs/promises").then((fs) =>
  fs.readFile(new URL("../src/bots/botsService.ts", import.meta.url), "utf8").catch(() => null),
);

function requireSource(): string {
  assert.ok(botsServiceSource, "无法读取 botsService 源码");
  return botsServiceSource!;
}

/** 取 `start` 标记之后的窗口（行窗口，避免跨函数误命中）。 */
function windowAfter(source: string, marker: string, lineSpan: number): string {
  const index = source.indexOf(marker);
  assert.ok(index >= 0, `未找到锚点：${marker}`);
  const tail = source.slice(index);
  return tail.split("\n").slice(0, lineSpan).join("\n");
}

/** 剥掉行注释：修复依据注释会提到被禁的旧写法，断言只看代码。 */
function stripLineComments(block: string): string {
  return block
    .split("\n")
    .filter((line) => !line.trim().startsWith("//"))
    .join("\n");
}

test("唯一目标函数：resolveOutboundTarget 定义并被 createOutbound 复用", () => {
  const source = requireSource();
  const definition = windowAfter(source, "function resolveOutboundTarget", 12);
  assert.ok(
    definition.includes("actor.chatId ?? actor.providerUserId"),
    "resolveOutboundTarget 必须按 chatId → providerUserId 取所属会话",
  );

  const createOutbound = windowAfter(source, "function createOutbound(", 20);
  assert.ok(
    createOutbound.includes("providerUserId: resolveOutboundTarget(actor)"),
    "createOutbound 必须复用同一个目标函数（禁止第二份手写表达式）",
  );
  assert.ok(
    !stripLineComments(createOutbound).includes("actor.chatId ?? actor.providerUserId"),
    "createOutbound 不得保留手写目标表达式",
  );
});

test("流式卡片目标：state.providerUserId 取所属会话，不是发送者", () => {
  const source = requireSource();
  const syncCard = windowAfter(source, "const syncStreamingCardReply", 90);
  assert.ok(
    syncCard.includes("providerUserId: resolveOutboundTarget(actor)"),
    "流式卡片状态必须用所属会话做投递目标（群 chatId / 私聊 userId）",
  );
  assert.ok(
    !stripLineComments(syncCard).includes("providerUserId: actor.providerUserId"),
    "禁止按发送者 providerUserId 建卡——企微群帧键是 chatid，错键会把终稿推进私聊",
  );
});

test("桌面输入镜像：附件不进镜像，但必须在 IM 侧显式告知未同步", () => {
  const source = requireSource();
  const mirror = windowAfter(source, "async notifyDesktopUserMessage", 110);
  assert.ok(mirror.includes("attachmentCount"), "镜像接口必须接收附件数量，否则无从告知");
  assert.ok(
    mirror.includes("desktopPromptMirrorAttachmentsSkipped"),
    "有附件时必须带上未同步提示——静默省略会让 IM 侧读者以为这次提问是完整的",
  );
  assert.ok(
    mirror.includes("+ attachmentsSkipped"),
    "提示必须接在镜像正文之后（同一条消息），不得另发一条",
  );
});

test("桌面输入镜像：目标是本对话，群回群、私聊回本人", () => {
  const source = requireSource();
  const mirror = windowAfter(source, "async notifyDesktopUserMessage", 70);
  assert.ok(
    mirror.includes("const mirrorUserId = context.conversationId"),
    "镜像目标必须是对话自身 id（私聊=对方 userId，群=群 chatId）",
  );
  assert.ok(
    !stripLineComments(mirror).includes("resolveBotProactiveUserId"),
    "镜像不得回落 resolveBotProactiveUserId——该函数首选绑定用户私聊（weixin-provider-self-id 规则 5）",
  );
});
