import assert from "node:assert/strict";
import test from "node:test";

// 心跳调度与安静回执的纯函数：决定"什么时候主动打扰用户"和"什么时候保持安静"。

test("normalizeBotHeartbeat：未开启返回 null；间隔裁剪到 15–1440 分钟；不再带活跃时段", async () => {
  const {
    normalizeBotHeartbeat,
    BOT_HEARTBEAT_MIN_INTERVAL_MINUTES,
    BOT_HEARTBEAT_MAX_INTERVAL_MINUTES,
    BOT_HEARTBEAT_DEFAULT_INTERVAL_MINUTES,
  } = await import("../src/bots/botHeartbeat.js");

  assert.equal(normalizeBotHeartbeat(undefined), null);
  assert.equal(normalizeBotHeartbeat({ enabled: false, intervalMinutes: 60 }), null);

  const tooSmall = normalizeBotHeartbeat({ enabled: true, intervalMinutes: 1 });
  assert.equal(tooSmall?.intervalMinutes, BOT_HEARTBEAT_MIN_INTERVAL_MINUTES);
  const tooLarge = normalizeBotHeartbeat({ enabled: true, intervalMinutes: 99999 });
  assert.equal(tooLarge?.intervalMinutes, BOT_HEARTBEAT_MAX_INTERVAL_MINUTES);
  const fallback = normalizeBotHeartbeat({ enabled: true, intervalMinutes: Number.NaN });
  assert.equal(fallback?.intervalMinutes, BOT_HEARTBEAT_DEFAULT_INTERVAL_MINUTES);
  // 活跃时段设置已下线：旧配置里的字段不会再被归一化结果带出来（写回时自然清除）。
  const legacy = normalizeBotHeartbeat({
    enabled: true,
    intervalMinutes: 60,
    activeHours: { start: "09:00", end: "21:00" },
  });
  assert.deepEqual(legacy, { enabled: true, intervalMinutes: 60 });
});

test("isHeartbeatDue：无记录视为到期；按间隔判断", async () => {
  const { isHeartbeatDue } = await import("../src/bots/botHeartbeat.js");
  const now = Date.now();
  assert.equal(isHeartbeatDue(now, { intervalMinutes: 60 }), true);
  assert.equal(
    isHeartbeatDue(now, { lastHeartbeatAt: now - 59 * 60_000, intervalMinutes: 60 }),
    false,
  );
  assert.equal(
    isHeartbeatDue(now, { lastHeartbeatAt: now - 61 * 60_000, intervalMinutes: 60 }),
    true,
  );
});

test("isHeartbeatOkOnly：仅回执时抑制；附带任何实质内容都照常发送", async () => {
  const { isHeartbeatOkOnly } = await import("../src/bots/botHeartbeat.js");
  assert.equal(isHeartbeatOkOnly("HEARTBEAT_OK"), true);
  assert.equal(isHeartbeatOkOnly("  heartBeat_ok \n"), true);
  assert.equal(isHeartbeatOkOnly("**HEARTBEAT_OK**"), true);
  assert.equal(isHeartbeatOkOnly("HEARTBEAT_OK\n\n构建失败，请查看日志。"), false);
  assert.equal(isHeartbeatOkOnly("没有需要汇报的内容"), false);
  assert.equal(isHeartbeatOkOnly(""), false);
});

// 心跳不写会话：提示词标记是 UI 隐藏整轮的判据（服务与 UI 共用同一份共享常量）。
test("isBotHeartbeatPromptText：只认心跳提示词开头，普通消息不受影响", async () => {
  const {
    isBotHeartbeatPromptText,
    BOT_HEARTBEAT_PROMPT_MARKER_ZH,
    BOT_HEARTBEAT_PROMPT_MARKER_EN,
  } = await import("@zcode/shared");
  assert.equal(isBotHeartbeatPromptText(`${BOT_HEARTBEAT_PROMPT_MARKER_ZH}。请查看当前工作区`), true);
  assert.equal(isBotHeartbeatPromptText(`  ${BOT_HEARTBEAT_PROMPT_MARKER_EN}. Review the workspace`), true);
  assert.equal(isBotHeartbeatPromptText("你好"), false);
  assert.equal(isBotHeartbeatPromptText("心跳正常吗？"), false);
  assert.equal(isBotHeartbeatPromptText("HEARTBEAT_OK"), false);
});

// 源码契约：服务侧提示词用共享标记拼装，UI 侧按标记剔除整轮心跳。
test("心跳不写会话：服务用共享标记生成提示词，UI 按标记隐藏整轮", async () => {
  const fs = await import("node:fs/promises");
  const service = await fs.readFile(new URL("../src/bots/botsService.ts", import.meta.url), "utf8");
  assert.ok(
    service.includes("BOT_HEARTBEAT_PROMPT_MARKER_ZH}。请查看当前工作区"),
    "心跳提示词必须由共享标记拼装（否则 UI 判据会与服务漂移）",
  );
  assert.ok(
    service.includes("BOT_HEARTBEAT_PROMPT_MARKER_EN}. Review the current workspace"),
    "英文心跳提示词同样用共享标记",
  );
  assert.ok(
    service.includes('msg(auth.locale, "heartbeatTaskTitle")'),
    "心跳新建任务用中性标题，避免侧栏出现心跳提示词当任务名",
  );
  const ui = await fs.readFile(
    new URL("../../../packages/ui/src/v4/conversationTurnRenderUnits.ts", import.meta.url),
    "utf8",
  );
  assert.ok(
    ui.includes("isBotHeartbeatPromptText(row.text)"),
    "UI 必须按心跳标记隐藏整轮",
  );
});
