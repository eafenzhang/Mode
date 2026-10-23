import assert from "node:assert/strict";
import test, { after } from "node:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

// 访问控制统一双模式（docs/specs/bot-access-mode.md R1/R5）：
// 1) 显式 accessMode 永远优先；
// 2) 无 accessMode 时按存量 privateChatMode 推导（all_users→free、bound_users→whitelist）；
// 3) 两者皆缺省时沿用旧 provider 缺省（微信 free、其余 whitelist）——存量不锁死；
// 4) 严格 schema 必须登记新字段，否则保存抛 ZodError。

// —— 服务级 harness（模式取自 test/bot-group-permission-service.test.ts）——
// 简报给的「每个用例 mkdtemp + 改 env + 重建服务」工厂在本进程内不可行：
// packages/services/src/paths.ts 在模块加载期就把 MODE_DATA_BASE_DIR 固化成模块级
// 常量（envDataBaseDir），测试 import 完 node 之后再改 env 不会切换数据目录；
// 同进程多次 createLocalServices 也没有被既有测试证明可用。因此按简报注明的
// 退化方案：模块级单数据目录 + 懒创建的单服务，每个用例直接重写
// bot-config.v3.json（BotsRepo.readConfig 每次读盘、无缓存——group-permission
// 服务级用例正是靠重写配置文件复用同一个服务）。
const dataDir = mkdtempSync(join(tmpdir(), "mode-bot-access-"));
process.env.MODE_DATA_BASE_DIR = dataDir;
process.env.MODE_DESKTOP_HOME_DIR = dataDir;

const repoRoot = fileURLToPath(new URL("../../../", import.meta.url));
const node = await import("@mode/services/node");
const services = await import("@mode/services");

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

async function getBotsService() {
  if (!hostServices) {
    hostServices = await node.createLocalServices({
      modeBuiltinProviderConfigFilePath: join(repoRoot, "config/provider/mode-builtin.json"),
    });
  }
  return hostServices.get(services.IBotsService);
}

/** 每个用例重写整份配置，隔离用例间的 bot/授权/登记状态。 */
async function writeBotsConfig(configBots: Array<Record<string, unknown>>) {
  const configDir = node.getAppConfigDir();
  mkdirSync(configDir, { recursive: true });
  writeFileSync(
    join(configDir, "bot-config.v3.json"),
    `${JSON.stringify({ version: 3, bots: configBots }, null, 2)}\n`,
  );
}

function baseBot(overrides: Record<string, unknown>) {
  return {
    id: "bot-acc",
    name: "bot-acc",
    // 关闭运行时连接不需要 enabled:false：group-permission 用例已证明
    // wecom 无凭据时 handleInboundMessage 只把回复当返回值，不发起真实连接。
    provider: "wecom",
    enabled: true,
    allowedWorkspaces: ["*"],
    allowedCommands: {
      status: true,
      new: true,
      workspace: true,
      model: true,
      mode: true,
      thoughtLevel: true,
      reply: true,
    },
    currentOptions: {},
    replyMode: "streaming_card",
    ...overrides,
  };
}

function inbound(overrides: Record<string, unknown>) {
  return {
    botId: "bot-acc",
    text: "hi",
    actor: {
      provider: "wecom",
      botId: "bot-acc",
      providerUserId: "u1",
      chatType: "private",
      providerMessageId: "m-acc",
      ...overrides,
    },
  } as never;
}

test("resolveBotAccessMode：显式 accessMode 优先于 privateChatMode", async () => {
  const { resolveBotAccessMode } = await import("@mode/shared");
  assert.equal(
    resolveBotAccessMode({
      provider: "weixin",
      accessMode: "whitelist",
      privateChatMode: "all_users",
    }),
    "whitelist",
  );
  assert.equal(
    resolveBotAccessMode({ provider: "wecom", accessMode: "free", privateChatMode: "bound_users" }),
    "free",
  );
});

test("resolveBotAccessMode：无 accessMode 时从存量 privateChatMode 推导", async () => {
  const { resolveBotAccessMode } = await import("@mode/shared");
  assert.equal(resolveBotAccessMode({ provider: "wecom", privateChatMode: "all_users" }), "free");
  assert.equal(
    resolveBotAccessMode({ provider: "weixin", privateChatMode: "bound_users" }),
    "whitelist",
  );
});

test("resolveBotAccessMode：两者皆缺省沿用旧 provider 缺省（存量微信不锁死）", async () => {
  const { resolveBotAccessMode } = await import("@mode/shared");
  assert.equal(resolveBotAccessMode({ provider: "weixin" }), "free");
  assert.equal(resolveBotAccessMode({ provider: "wecom" }), "whitelist");
  assert.equal(resolveBotAccessMode({ provider: "feishu" }), "whitelist");
  assert.equal(resolveBotAccessMode({ provider: "dingtalk" }), "whitelist");
});

test("botConfigSchema：新增字段能严格解析并往返", async () => {
  const { botsConfigFileSchema } = await import("@mode/shared");
  const parsed = botsConfigFileSchema.parse({
    version: 3,
    bots: [
      {
        id: "bot-1",
        name: "b",
        provider: "wecom",
        enabled: true,
        accessMode: "free",
        blockedUsers: ["u-black"],
        blockedGroups: [
          { groupId: "g-black", groupName: "Bad group", platform: "wecom", blockedAt: 1 },
        ],
        userRegistry: [{ providerUserId: "u1", displayName: "Alice", discoveredAt: 2 }],
        allowedWorkspaces: ["*"],
        // 简报原样写的是 {}，但 botAllowedCommandsSchema 有 6 个必填布尔
        // （status/new/workspace/model/thoughtLevel/reply），缺一会先于本任务字段报错；
        // 这里补齐以聚焦本次新增的 4 个字段能否通过 strict 校验。
        allowedCommands: {
          status: true,
          new: true,
          workspace: true,
          model: true,
          thoughtLevel: true,
          reply: true,
        },
        currentOptions: {},
        replyMode: "assistant_changes",
      },
    ],
  });
  assert.equal(parsed.bots[0]?.accessMode, "free");
  assert.equal(parsed.bots[0]?.blockedUsers?.[0], "u-black");
  assert.equal(parsed.bots[0]?.blockedGroups?.[0]?.groupId, "g-black");
  assert.equal(parsed.bots[0]?.userRegistry?.[0]?.providerUserId, "u1");
});

test("findBoundUser：free 模式任何私聊用户放行，whitelist 按绑定收紧", async () => {
  const { findBoundUser } = await import("../src/bots/botConfigHelpers.js");
  const free = {
    id: "b1",
    provider: "wecom",
    providerUserId: "owner",
    allowedUsers: ["t1"],
    accessMode: "free",
  } as never;
  const whitelist = { ...free, accessMode: "whitelist" } as never;
  const actor = (providerUserId: string) =>
    ({ provider: "wecom", botId: "b1", providerUserId, chatType: "private" }) as never;
  assert.ok(findBoundUser(free, actor("stranger")), "free 模式陌生私聊必须放行");
  assert.equal(findBoundUser(whitelist, actor("stranger")), null, "whitelist 必须拒绝陌生私聊");
  assert.ok(findBoundUser(whitelist, actor("t1")), "whitelist 放行 allowedUsers");
});

test("黑名单纯函数：按 bot 配置精确命中，缺席不误伤", async () => {
  const { isUserBlocked, isGroupBlocked } = await import("../src/bots/botConfigHelpers.js");
  const bot = {
    id: "b1",
    provider: "wecom",
    blockedUsers: ["u-black"],
    blockedGroups: [{ groupId: "g-black", platform: "wecom", blockedAt: 1 }],
  } as never;
  assert.equal(isUserBlocked(bot, "u-black"), true);
  assert.equal(isUserBlocked(bot, "u-ok"), false);
  assert.equal(isGroupBlocked(bot, "g-black"), true);
  assert.equal(isGroupBlocked(bot, "g-ok"), false);
  assert.equal(
    isUserBlocked({ id: "b1", provider: "wecom" } as never, "u-any"),
    false,
    "无字段视为未拉黑",
  );
});

test("normalizeBotConfig：回填 accessMode（存量推导不锁死，显式值原样保留）", async () => {
  const { normalizeBotConfig } = await import("../src/bots/botConfigHelpers.js");
  const base = {
    id: "b1",
    name: "b",
    provider: "weixin",
    enabled: true,
    allowedWorkspaces: ["*"],
    // 简报原样写的是 {}，但 botAllowedCommandsSchema 有 6 个必填布尔
    // （status/new/workspace/model/thoughtLevel/reply）；此处补齐以与上方 schema 用例一致。
    allowedCommands: {
      status: true,
      new: true,
      workspace: true,
      model: true,
      thoughtLevel: true,
      reply: true,
    },
    currentOptions: {},
    replyMode: "assistant_changes",
  } as never;
  assert.equal(normalizeBotConfig(base).accessMode, "free", "存量微信无字段推导 free");
  assert.equal(
    normalizeBotConfig({ ...base, accessMode: "whitelist" } as never).accessMode,
    "whitelist",
    "显式值原样保留",
  );
  assert.equal(
    normalizeBotConfig({ ...base, provider: "wecom", privateChatMode: "all_users" } as never)
      .accessMode,
    "free",
    "存量 all_users 推导 free",
  );
});

// —— 服务级准入闸门与 free 自动登记（docs/specs/bot-access-mode.md R2/R3/R4）——

test("黑名单：私聊用户在两种模式下都静默拒绝，且不登记 userRegistry", async () => {
  for (const accessMode of ["whitelist", "free"] as const) {
    await writeBotsConfig([baseBot({ accessMode, blockedUsers: ["u-black"] })]);
    const botsService = await getBotsService();
    assert.deepEqual(
      await botsService.handleInboundMessage(inbound({ providerUserId: "u-black" })),
      [],
      `${accessMode} 下被拉黑用户必须静默`,
    );
    const config = await botsService.getConfig();
    assert.equal(
      config.bots.find((b) => b.id === "bot-acc")?.userRegistry?.length ?? 0,
      0,
      "被拉黑消息不得登记",
    );
  }
});

test("黑名单：已批准群被拉黑后静默（黑名单优先于授权，spec R2）", async () => {
  await writeBotsConfig([
    baseBot({
      accessMode: "whitelist",
      blockedGroups: [{ groupId: "g-live", platform: "wecom", blockedAt: 1 }],
      groupChat: {
        activation: "always",
        permissions: [
          {
            groupId: "g-live",
            groupName: "Live",
            platform: "wecom",
            status: "approved",
            discoveredAt: 1,
          },
        ],
      },
    }),
  ]);
  const botsService = await getBotsService();
  assert.deepEqual(
    await botsService.handleInboundMessage(
      inbound({ chatType: "group", chatId: "g-live", providerUserId: "u1" }),
    ),
    [],
  );
});

test("free 模式：私聊首条通过准入的消息自动登记 userRegistry，第二条不重复", async () => {
  await writeBotsConfig([baseBot({ accessMode: "free" })]);
  const botsService = await getBotsService();
  await botsService.handleInboundMessage(
    inbound({ providerUserId: "u-new", displayName: "Alice" }),
  );
  await botsService.handleInboundMessage(
    inbound({ providerUserId: "u-new", providerMessageId: "m2" }),
  );
  const bot = (await botsService.getConfig()).bots.find((b) => b.id === "bot-acc");
  assert.equal(bot?.userRegistry?.length, 1, "同一用户只登记一次");
  assert.equal(bot?.userRegistry?.[0]?.providerUserId, "u-new");
  assert.equal(bot?.userRegistry?.[0]?.displayName, "Alice");
});

test("free 模式：群首条消息免批登记 approved+auto，直接进入触发链路", async () => {
  await writeBotsConfig([baseBot({ accessMode: "free", groupChat: { activation: "always" } })]);
  const botsService = await getBotsService();
  // 无工作区 → 停在 noWorkspaceAllowed 回复；非空回复即证明没有被 pending 闸门静默。
  const replies = await botsService.handleInboundMessage(
    inbound({ chatType: "group", chatId: "g-free", providerUserId: "u1" }),
  );
  assert.ok(replies.length > 0, "free 群免批必须进入触发链路");
  const permission = (await botsService.getConfig()).bots
    .find((b) => b.id === "bot-acc")
    ?.groupChat?.permissions?.find((p) => p.groupId === "g-free");
  assert.equal(permission?.status, "approved");
  assert.equal(permission?.addedBy, "auto");
});

test("whitelist 模式：群发现仍登记 pending 并静默（A+ 行为不回退）", async () => {
  await writeBotsConfig([
    baseBot({ accessMode: "whitelist", groupChat: { activation: "always" } }),
  ]);
  const botsService = await getBotsService();
  const replies = await botsService.handleInboundMessage(
    inbound({ chatType: "group", chatId: "g-wl", providerUserId: "stranger" }),
  );
  assert.deepEqual(replies, [], "whitelist 未批准群必须静默");
  const permission = (await botsService.getConfig()).bots
    .find((b) => b.id === "bot-acc")
    ?.groupChat?.permissions?.find((p) => p.groupId === "g-wl");
  assert.equal(permission?.status, "pending");
});
