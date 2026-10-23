import assert from "node:assert/strict";
import test from "node:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// 跨聊天交接协议 schema（docs/specs/bot-cross-chat-handoff.md R1/R5）：
// strict 拒绝多余键；状态枚举与 toolCallId 必填，保证幂等键始终存在。

test("modeBotHandoffParamsSchema：合法参数通过，多余键被拒绝", async () => {
  const { modeBotHandoffParamsSchema } = await import("@mode/shared");
  const base = {
    target: { provider: "wecom", botId: "b1", providerUserId: "u1", chatType: "private" },
    conversationKey: "private:u2",
    message: "请转达",
    toolCallId: "call-1",
  };
  assert.deepEqual(modeBotHandoffParamsSchema.parse(base), base);
  assert.throws(() => modeBotHandoffParamsSchema.parse({ ...base, extra: 1 }), /unrecognized/i);
  assert.throws(() => modeBotHandoffParamsSchema.parse({ ...base, toolCallId: "" }), /./);
  assert.throws(() => modeBotHandoffParamsSchema.parse({ ...base, message: "" }), /./);
});

test("modeBotHandoffResultSchema：三态 + 目标回显", async () => {
  const { modeBotHandoffResultSchema } = await import("@mode/shared");
  for (const status of ["submitted", "queued", "duplicate"] as const) {
    assert.equal(
      modeBotHandoffResultSchema.parse({ status, conversationKey: "private:u2" }).status,
      status,
    );
  }
  assert.throws(() => modeBotHandoffResultSchema.parse({ status: "lost", conversationKey: "x" }));
});

test("buildHandoffInboundPrompt：system-reminder 包装、含来源/背景、正文在最后、不以 / 开头", async () => {
  const { buildHandoffInboundPrompt } = await import("../src/bots/handoffPrompt.js");
  const out = buildHandoffInboundPrompt({
    source: "private:u1（Alice）",
    context: "用户在私聊里让我转达",
    content: "目标会话正文",
    locale: "zh-CN",
  });
  assert.ok(out.startsWith("<system-reminder>"), "必须是 reminder 包装（展示层 strip 依赖）");
  assert.ok(out.includes("private:u1（Alice）"), "必须含来源");
  assert.ok(out.includes("用户在私聊里让我转达"), "必须含背景");
  assert.ok(out.trimEnd().endsWith("目标会话正文"), "正文在最后");
  assert.ok(!out.split("</system-reminder>")[1]?.trim().startsWith("/"), "正文不得被解析成命令");
  // 无背景时省略背景行
  const minimal = buildHandoffInboundPrompt({ source: "group:g1", content: "x" });
  assert.ok(!minimal.includes("undefined"));
});

test("formatHandoffTargets：列出键与显示名，无显示名回退 conversationId", async () => {
  const { formatHandoffTargets } = await import("../src/bots/handoffPrompt.js");
  const out = formatHandoffTargets(
    [
      { conversationKey: "private:u2", conversationLabel: "Bob", conversationId: "u2" },
      { conversationKey: "group:g1", conversationId: "g1" },
    ],
    "zh-CN",
  );
  assert.match(out, /private:u2/);
  assert.match(out, /Bob/);
  assert.match(out, /group:g1/);
  assert.match(out, /<system-reminder>/);
  assert.ok(!out.includes("undefined"));
});

test("resolveValidatedMediaFile：tmp 内合法文件通过，白名单外拒绝", async () => {
  const { resolveValidatedMediaFile } = await import("../src/bots/botMedia.js");
  const dir = mkdtempSync(join(tmpdir(), "bot-handoff-media-"));
  const file = join(dir, "ok.png");
  writeFileSync(file, "x");
  const ok = await resolveValidatedMediaFile(join(dir, "ws"), file);
  assert.equal(ok.filename, "ok.png");
  assert.equal(ok.kind, "image");
  // 白名单外：root 指向 dir，文件放在白名单三根（该根 / app data / tmp）之外。
  // 不能放在"另一个 tmp 子目录"——tmp 本身在白名单内（/send 明确允许系统临时目录，
  // 错误文案也写死 temp），tmp 内任何文件都拒不掉；cwd 位于仓库内，不落在
  // tmp/app data/workspace 任一根下，且各平台可写，放这里并用完即删。
  const outsideDir = mkdtempSync(join(process.cwd(), "bot-handoff-out-"));
  try {
    const outside = join(outsideDir, "nope.png");
    writeFileSync(outside, "x");
    await assert.rejects(
      () => resolveValidatedMediaFile(dir, outside),
      /Path is outside the allowed directories/,
    );
  } finally {
    rmSync(outsideDir, { recursive: true, force: true });
  }
  await assert.rejects(
    () => resolveValidatedMediaFile(dir, join(dir, "missing.png")),
    /File not found/,
  );
});
