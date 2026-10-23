import assert from "node:assert/strict";
import test, { after } from "node:test";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

// 跨聊天交接协议 schema（docs/specs/bot-cross-chat-handoff.md R1/R5）：
// strict 拒绝多余键；状态枚举与 toolCallId 必填，保证幂等键始终存在。

// —— 服务级 harness（模式取自 test/bot-group-permission-service.test.ts /
//    test/bot-access-mode.test.ts）：paths.ts 在模块加载期固化 MODE_DATA_BASE_DIR，
//    必须在 import node 之前设好；同进程单服务 + 直接改写落盘文件复用（repo 每次读盘）。 ——
const dataDir = mkdtempSync(join(tmpdir(), "mode-bot-handoff-"));
process.env.MODE_DATA_BASE_DIR = dataDir;
process.env.MODE_DESKTOP_HOME_DIR = dataDir;

const repoRoot = fileURLToPath(new URL("../../../", import.meta.url));
const node = await import("@mode/services/node");
const services = await import("@mode/services");

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

// —— 服务级用例（spec 验收场景 1 的可确定性分支）——
// 说明：submitted/queued 终态与「目标回合回复出现在目标聊天」需要真实 Workspace +
// 模型链路，hermetic 下不可达，归入 spec 验收场景 3（实机）；本文件覆盖 spec
// 验收场景 1 的全部可确定性分支。

let hostServices: Awaited<ReturnType<typeof node.createLocalServices>> | null = null;

after(async () => {
  if (hostServices) {
    await node.disposeServiceResourcesAndWait(hostServices).catch(() => undefined);
  }
  try {
    rmSync(dataDir, { recursive: true, force: true });
  } catch {
    // best-effort；Windows 句柄晚释放不应把测试判红。
  }
});

/** 首次使用时创建服务（配置/状态文件须在 createLocalServices 之前写入初值——
 *  因此本文件在模块顶层先写一份空 bots 配置，再由 ensureServices 在首个用例里建服务；
 *  后续用例只改文件，repo 每次读取重读磁盘）。 */
async function ensureServices(): Promise<NonNullable<typeof hostServices>> {
  if (hostServices) return hostServices;
  const { BOTS_STATE_FILE } = await import("../src/bots/config.js"); // 实际导出名以 config.ts 为准
  const file = join(node.getAppConfigDir(), "bot-config.v3.json");
  if (!existsSync(file)) {
    mkdirSync(node.getAppConfigDir(), { recursive: true });
    writeFileSync(file, `${JSON.stringify({ version: 3, bots: [] }, null, 2)}\n`);
  }
  const stateFile = join(node.getAppConfigDir(), BOTS_STATE_FILE);
  if (!existsSync(stateFile)) {
    writeFileSync(stateFile, `${JSON.stringify({ version: 4, bots: {} }, null, 2)}\n`);
  }
  hostServices = await node.createLocalServices({
    modeBuiltinProviderConfigFilePath: join(repoRoot, "config/provider/mode-builtin.json"),
  });
  return hostServices;
}

const configDir = () => node.getAppConfigDir();
function tmpWs(): string {
  return mkdtempSync(join(tmpdir(), "bot-handoff-ws-"));
}
function baseBot(botId: string, overrides: Record<string, unknown>) {
  return {
    id: botId,
    name: botId,
    provider: "wecom",
    enabled: true,
    allowedWorkspaces: ["*"],
    allowedCommands: { status: true, new: true, workspace: true, model: true, mode: true, thoughtLevel: true, reply: true },
    currentOptions: {},
    replyMode: "streaming_card",
    ...overrides,
  };
}
/** 直接改写配置文件追加一个 bot（服务只建一次；读取走 repo 每次重读文件）。
 *  允许在 ensureServices 之前调用：文件不存在时先建空配置。 */
async function seedBotConfig(bot: Record<string, unknown>): Promise<void> {
  const file = join(configDir(), "bot-config.v3.json");
  if (!existsSync(file)) {
    mkdirSync(configDir(), { recursive: true });
    writeFileSync(file, `${JSON.stringify({ version: 3, bots: [] }, null, 2)}\n`);
  }
  const current = JSON.parse(readFileSync(file, "utf8")) as { version: number; bots: Record<string, unknown>[] };
  current.bots = current.bots.filter((item) => item.id !== bot.id);
  current.bots.push(bot);
  writeFileSync(file, `${JSON.stringify(current, null, 2)}\n`);
}
/** 状态文件整体重写为「仅包含这些 bot 的对话种子」（用例间互不串扰）。 */
async function seedConversations(stateBots: Record<string, unknown>): Promise<void> {
  const { botsStateFileSchema } = await import("@mode/shared");
  const { BOTS_STATE_FILE } = await import("../src/bots/config.js"); // 实际导出名以 config.ts 为准
  const parsed = botsStateFileSchema.parse({ version: 4, bots: stateBots });
  writeFileSync(join(configDir(), BOTS_STATE_FILE), `${JSON.stringify(parsed, null, 2)}\n`);
}
function handoffParams(botId: string, overrides: Record<string, unknown>) {
  return {
    target: { provider: "wecom" as const, botId, providerUserId: "u1", chatType: "private" as const },
    conversationKey: "private:u2",
    message: "hi",
    toolCallId: "call-1",
    ...overrides,
  } as never;
}
/** 每个 bot 独立对话种子：conversation 必填字段见 BotConversationState schema */
function seedConversation(botId: string, key: string, workspacePath: string) {
  return {
    [botId]: {
      botId,
      conversations: {
        [key]: {
          botId,
          conversationKey: key,
          conversationKind: key.startsWith("group:") ? "group" : "private",
          conversationId: key.split(":").slice(1).join(":"),
          workspacePath,
          mode: "draft",
          activeTaskId: null,
          updatedAt: Date.now(),
        },
      },
      updatedAt: Date.now(),
    },
  };
}
/** 用例标准布局：free bot + 源对话 private:u1 + 目标对话 private:u2。返回唯一 botId。 */
async function seedFreePair(options?: {
  u2Overrides?: Record<string, unknown>;
  botOverrides?: Record<string, unknown>;
}): Promise<string> {
  const botId = `bot-ho-${Math.random().toString(36).slice(2, 8)}`;
  await seedBotConfig(baseBot(botId, { accessMode: "free", ...(options?.botOverrides) }));
  const ws = tmpWs();
  const source = seedConversation(botId, "private:u1", ws);
  const target = seedConversation(botId, "private:u2", options?.u2Overrides?.["workspacePath"] as string ?? ws);
  const merged = {
    [botId]: {
      botId,
      conversations: {
        ...(source[botId] as { conversations: Record<string, unknown> }).conversations,
        ...(target[botId] as { conversations: Record<string, unknown> }).conversations,
      },
      updatedAt: Date.now(),
    },
  };
  await seedConversations(merged);
  return botId;
}

test("handoffMessage：目标不存在报错并回显可用清单", async () => {
  const botId = await seedFreePair();
  const botsService = (await ensureServices()).get(services.IBotsService);
  await assert.rejects(
    () =>
      botsService.handoffMessage(
        handoffParams(botId, { conversationKey: "private:ghost", toolCallId: "call-miss" }),
      ),
    (error: Error) =>
      error.message.includes("private:ghost") && error.message.includes("private:u1"),
  );
});

test("handoffMessage：源目标同对话 / 非法键 / 黑名单 / 群未授权 均拒绝", async () => {
  const botsService = (await ensureServices()).get(services.IBotsService);

  // 1) 目标 === 源对话  2) 非法键
  const pairBot = await seedFreePair();
  await assert.rejects(
    () => botsService.handoffMessage(handoffParams(pairBot, { conversationKey: "private:u1" })),
    /current conversation/,
  );
  await assert.rejects(
    () => botsService.handoffMessage(handoffParams(pairBot, { conversationKey: "weird" })),
    /Invalid target conversation key/,
  );

  // 3) 黑名单（计划①字段）：目标 u2 已拉黑
  const blockedBot = await seedFreePair({ botOverrides: { blockedUsers: ["u2"] } });
  await assert.rejects(() => botsService.handoffMessage(handoffParams(blockedBot)), /blocked/);

  // 4) 群目标：whitelist 无批准记录 → not authorized（先种子 group 对话再调用）。
  // 群必须显式非 disabled 的 activation，否则先撞 disabled 闸门（用例 5 专测 disabled）。
  const whitelistBot = await seedFreePair({
    botOverrides: { accessMode: "whitelist", groupChat: { activation: "mention" } },
  });
  const ws = tmpWs();
  await seedConversations({
    [whitelistBot]: {
      botId: whitelistBot,
      conversations: {
        "private:u1": {
          botId: whitelistBot, conversationKey: "private:u1", conversationKind: "private",
          conversationId: "u1", workspacePath: ws, mode: "draft", activeTaskId: null, updatedAt: Date.now(),
        },
        "group:g2": {
          botId: whitelistBot, conversationKey: "group:g2", conversationKind: "group",
          conversationId: "g2", workspacePath: ws, mode: "draft", activeTaskId: null, updatedAt: Date.now(),
        },
      },
      updatedAt: Date.now(),
    },
  });
  await assert.rejects(
    () => botsService.handoffMessage(handoffParams(whitelistBot, { conversationKey: "group:g2" })),
    /not authorized/,
  );

  // 5) 群目标 disabled：种子 group 对话 + activation disabled → disabled
  const disabledBot = await seedFreePair({ botOverrides: { groupChat: { activation: "disabled" } } });
  await seedConversations({
    [disabledBot]: {
      botId: disabledBot,
      conversations: {
        "private:u1": {
          botId: disabledBot, conversationKey: "private:u1", conversationKind: "private",
          conversationId: "u1", workspacePath: ws, mode: "draft", activeTaskId: null, updatedAt: Date.now(),
        },
        "group:g3": {
          botId: disabledBot, conversationKey: "group:g3", conversationKind: "group",
          conversationId: "g3", workspacePath: ws, mode: "draft", activeTaskId: null, updatedAt: Date.now(),
        },
      },
      updatedAt: Date.now(),
    },
  });
  await assert.rejects(
    () => botsService.handoffMessage(handoffParams(disabledBot, { conversationKey: "group:g3" })),
    /disabled/,
  );
});

test("handoffMessage：派发链路在 hermetic 下必拒，且错误不是任何预检串（越过目标校验的证据）", async () => {
  // workspacePath 指向已删除目录的种子对话：readContext 对「state 中已存在」的对话
  // 必返回非空（noWorkspace 预检不可达），派发必然在模型/选项解析处抛错
  // 或回非空拒绝回复 → handoffMessage rejects。
  const botId = await seedFreePair({
    u2Overrides: { workspacePath: join(tmpdir(), `gone-${Date.now()}`) },
  });
  const botsService = (await ensureServices()).get(services.IBotsService);
  await assert.rejects(
    () => botsService.handoffMessage(handoffParams(botId)),
    (error: Error) =>
      !/not found|Invalid target|blocked|disabled|not authorized|current conversation/.test(
        error.message,
      ),
  );
});

test("handoffMessage：幂等键失败释放——连续两次同 toolCallId 均抛同一错误（非静默 duplicate）", async () => {
  // 破损对话场景连调两次（同 toolCallId）：
  // 两次都 rejects 且 error.message 相同 → 证明首次失败已 release 幂等键（spec R5）；
  // 若第二次返回 {status:"duplicate"} 而非抛错，即为未释放，用例失败。
  const botId = await seedFreePair({
    u2Overrides: { workspacePath: join(tmpdir(), `gone-${Date.now()}`) },
  });
  const botsService = (await ensureServices()).get(services.IBotsService);
  const params = handoffParams(botId, { toolCallId: "call-r1" });
  const first = await botsService.handoffMessage(params).then(
    () => null,
    (error: Error) => error.message,
  );
  const second = await botsService.handoffMessage(params).then(
    () => null,
    (error: Error) => error.message,
  );
  assert.ok(first, "第一次必须失败（hermetic 无模型链路）");
  assert.equal(second, first, "第二次必须重试并抛同一错误，不得静默 duplicate");
});

test("handoffMessage：非法附件在任何投递前整次拒绝", async () => {
  // 有效目标 + attachments: ["/definitely/missing.png"]
  // 断言 rejects（/File not found|outside/）且第二次换合法附件调用能走到派发失败错误
  // → 证明去重标记未被校验失败消费（spec R4 整次拒绝 + spec R5 释放语义）。
  const botId = await seedFreePair();
  const botsService = (await ensureServices()).get(services.IBotsService);
  await assert.rejects(
    () =>
      botsService.handoffMessage(
        handoffParams(botId, {
          conversationKey: "private:u2",
          attachments: ["/definitely/missing.png"],
        }),
      ),
    /File not found|outside the allowed directories/,
  );
  const afterInvalid = await botsService
    .handoffMessage(handoffParams(botId, { conversationKey: "private:u2", toolCallId: "call-at" }))
    .then(
      () => null,
      (error: Error) => error.message,
    );
  assert.ok(afterInvalid, "合法附件的首次调用必须进入派发链路（不被上次失败卡成 duplicate）");
  assert.doesNotMatch(afterInvalid!, /File not found/);
});

test("handleMessage：交接注入与目标清单挂在标题推导之后、非心跳分支", () => {
  const source = readFileSync(
    new URL("../src/bots/botsService.ts", import.meta.url),
    "utf8",
  );
  // 注入发生在 titleSource 推导之后（保证任务标题取原始 text）
  const titleIndex = source.indexOf("const titleSource");
  const handoffIndex = source.indexOf("buildHandoffInboundPrompt(");
  const targetsIndex = source.indexOf("formatHandoffTargets(");
  assert.ok(titleIndex > 0 && handoffIndex > titleIndex, "交接注入必须在标题推导之后");
  assert.ok(targetsIndex > handoffIndex, "目标清单在交接包装之后");
  assert.match(source, /message\.handoff/);
  assert.match(source, /options\.heartbeat !== true/, "心跳回合不注入目标清单");
  assert.match(source, /enqueueInboundProcessing\(targetActor/, "交接按目标对话串行准入");
});
