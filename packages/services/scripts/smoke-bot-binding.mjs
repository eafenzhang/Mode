/**
 * Bot 绑定/焦点同步 冒烟测试（真实服务图，一次性数据目录）。
 * 用法（仓库根目录）：node --import tsx packages/services/scripts/smoke-bot-binding.mjs
 * 验证：绑定自动补全 allowedWorkspaces → 绑定可查询 → UI 会话焦点写入 bot 上下文 →
 * 幂等短路（重复通知不改状态）→ 会话跟随 → 草稿保护 → 解绑清空。不含 IM 网络调用。
 */
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const dataBaseDir = mkdtempSync(join(tmpdir(), "zcodium-bot-smoke-"));
process.env.ZCODE_DATA_BASE_DIR = dataBaseDir;
process.env.ZCODIUM_DATA_BASE_DIR = dataBaseDir;

const { IBotsService, collectServiceMemoryDiagnostics } = await import("@zcode/services");
const { createLocalServices } = await import("@zcode/services/node");

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
const services = await createLocalServices({
  zcodeBuiltinProviderConfigFilePath: join(
    repoRoot,
    "config/provider/zcode-builtin.json",
  ),
});
const botsService = services.get(IBotsService);

const workspacePath = join(dataBaseDir, "smoke-workspace");
const bot = await botsService.saveBot({
  bot: {
    id: "smoke-bot",
    name: "SmokeBot",
    provider: "webhook",
    enabled: true,
    providerUserId: "smoke-user-1",
    allowedWorkspaces: [],
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
    replyMode: "assistant_changes",
  },
});

// 1. 绑定：绑定即授权——allowedWorkspaces 收敛为该 bot 的绑定集
await botsService.bindBotToWorkspace({ botId: bot.id, workspacePath });
const binding = await botsService.getWorkspaceBotBinding({ workspacePath });
assert.deepEqual(binding.botIds, ["smoke-bot"]);
const saved = (await botsService.listBots()).find((item) => item.id === bot.id);
assert.deepEqual(saved.allowedWorkspaces, [workspacePath], "bind should converge allowedWorkspaces to the bound set");

// 1b. listBotWorkspaceBindings 返回该 bot 的绑定工作区（合成工作区也如实返回）
const bindings = await botsService.listBotWorkspaceBindings({ botId: bot.id });
assert.equal(bindings.length, 1);
assert.equal(bindings[0].id, workspacePath);
assert.deepEqual(await botsService.listBotWorkspaceBindings({ botId: "other-bot" }), []);

// 2. 全新的 bot 还没有任何对话：UI 会话焦点不会凭空造出上下文（首条入站消息才建立）
await botsService.notifyUiSessionFocus({ workspacePath, taskId: "task-1" });
assert.equal(
  (await botsService.getBotStates()).filter((item) => item.botId === "smoke-bot").length,
  0,
  "无对话的 bot 不应产生上下文",
);

// 3. 显式会话绑定（右键菜单/标题栏「···」路径）才建立会话归属；
//    未指定对话时按 bot 配置推导默认对话 = 绑定用户的私聊。
await botsService.bindBotToTask({ botId: bot.id, workspacePath, taskId: "task-1" });
let states = await botsService.getBotStates();
let state = states.find((item) => item.botId === "smoke-bot");
assert.equal(state.conversationKey, "private:smoke-user-1", "默认对话=绑定用户的私聊");
assert.equal(state.conversationKind, "private");
assert.equal(state.mode, "task");
assert.equal(state.activeTaskId, "task-1");
assert.equal(state.workspacePath, workspacePath);

// 4. 绑定后桌面再切会话：绑定必须稳住（绿点不能跟着选中跑）
const boundAt = state.updatedAt;
await new Promise((resolveSleep) => setTimeout(resolveSleep, 5));
await botsService.notifyUiSessionFocus({ workspacePath, taskId: "task-2" });
await botsService.notifyUiSessionFocus({ workspacePath, taskId: null });
states = await botsService.getBotStates();
state = states.find((item) => item.botId === "smoke-bot");
assert.equal(state.activeTaskId, "task-1", "已绑定的会话不能被桌面焦点夺走");
assert.equal(state.updatedAt, boundAt, "焦点通知不应改写已有会话绑定");

// 5. 草稿 bot 仍跟随工作区切换（IM 消息落在当前工作区）
const followBot = await botsService.saveBot({
  bot: {
    id: "smoke-bot-follow",
    name: "SmokeBotFollow",
    provider: "webhook",
    enabled: true,
    providerUserId: "smoke-user-3",
    allowedWorkspaces: [],
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
    replyMode: "assistant_changes",
  },
});
const followWorkspaceA = join(dataBaseDir, "smoke-follow-a");
const followWorkspaceB = join(dataBaseDir, "smoke-follow-b");
await botsService.bindBotToWorkspace({ botId: followBot.id, workspacePath: followWorkspaceA });
// 先建立对话再解绑 → 得到一个草稿对话（模拟"聊过但已解绑"）
await botsService.bindBotToTask({
  botId: followBot.id,
  conversationKey: "private:smoke-user-3",
  workspacePath: followWorkspaceA,
  taskId: "task-9",
});
await botsService.unbindBotFromTask({
  botId: followBot.id,
  taskId: "task-9",
  conversationKey: "private:smoke-user-3",
});
// 换绑工作区后焦点：草稿对话跟随新工作区，但不得被自动绑到会话上
await botsService.unbindBotFromWorkspace({ botId: followBot.id, workspacePath: followWorkspaceA });
await botsService.bindBotToWorkspace({ botId: followBot.id, workspacePath: followWorkspaceB });
await botsService.notifyUiSessionFocus({ workspacePath: followWorkspaceB, taskId: null });
states = await botsService.getBotStates();
const otherState = states.find((item) => item.botId === followBot.id);
assert.equal(otherState.workspacePath, followWorkspaceB, "草稿对话跟随工作区切换");
assert.equal(otherState.mode, "draft", "焦点不得把草稿对话自动绑到会话上");

// 6. 解绑 → 该 bot 从绑定表移除
await botsService.unbindBotFromWorkspace({ botId: bot.id, workspacePath });
assert.deepEqual(
  (await botsService.getWorkspaceBotBinding({ workspacePath })).botIds,
  [],
);
// 全部解绑后不自动放开授权：allowedWorkspaces 保持收敛结果，绑定列表清空。
assert.deepEqual(await botsService.listBotWorkspaceBindings({ botId: bot.id }), []);
const afterUnbind = (await botsService.listBots()).find((item) => item.id === bot.id);
assert.deepEqual(afterUnbind.allowedWorkspaces, [workspacePath], "unbind must not widen access");

console.log("SMOKE OK: bind → focus-no-steal → explicit-bind → sticky-binding → draft-follows-workspace → unbind 全部通过");
// ===== 会话级绑定 RPC（桌面右键菜单路径）：绑定 → 查询 → 桌面镜像容错 → 解绑 =====
await botsService.bindBotToTask({
  botId: bot.id,
  workspacePath,
  taskId: "task-desktop-1",
});
let taskBindings = await botsService.listBotTaskBindings();
const bound = taskBindings.find((item) => item.taskId === "task-desktop-1");
assert.ok(bound, "bindBotToTask 后应出现在绑定列表");
assert.equal(bound.botId, bot.id);

// 桌面输入镜像：provider 发送失败必须被吞掉（fire-and-forget，不能打断桌面发送）
await botsService.notifyDesktopUserMessage({
  taskId: "task-desktop-1",
  workspacePath,
  text: "来自桌面的输入",
});

const botStreamSubs = () => collectServiceMemoryDiagnostics()["bots.streamSubs"] ?? 0;
assert.equal(botStreamSubs(), 1, "绑定会话后应建立 1 条流订阅");

// ===== 多机器人：一个工作区绑定多个 bot，同一会话也可绑定多个 =====
const otherBot = await botsService.saveBot({
  bot: {
    id: "smoke-bot-2",
    name: "SmokeBot2",
    provider: "webhook",
    enabled: true,
    providerUserId: "smoke-user-2",
    allowedWorkspaces: [],
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
    replyMode: "assistant_changes",
  },
});

// 通配范围（含空数组）的 bot 不属于任何工作区：不允许绑到会话上
const rejected = await botsService.bindBotToTask({
  botId: otherBot.id,
  workspacePath,
  taskId: "task-desktop-2",
});
assert.deepEqual(rejected, { ok: false, reason: "workspace" }, "非本工作区的 bot 必须被拒绝");

// 把 bot2 与 bot 一起绑定到工作区：绑定是追加语义，不是替换
await botsService.bindBotToWorkspace({ botId: otherBot.id, workspacePath });
await botsService.bindBotToWorkspace({ botId: bot.id, workspacePath });
const workspaceBinding = await botsService.getWorkspaceBotBinding({ workspacePath });
assert.deepEqual(
  [...workspaceBinding.botIds].sort(),
  ["smoke-bot", "smoke-bot-2"],
  "一个工作区应能绑定多个 bot",
);
// 幂等：重复绑定同一 bot 不产生重复项
await botsService.bindBotToWorkspace({ botId: bot.id, workspacePath });
assert.equal((await botsService.getWorkspaceBotBinding({ workspacePath })).botIds.length, 2);

// 同一会话绑定两个 bot：各自镜像，互不挤掉
const accepted = await botsService.bindBotToTask({
  botId: otherBot.id,
  workspacePath,
  taskId: "task-desktop-1",
});
assert.equal(accepted.ok, true, "工作区绑定的 bot 应可绑定会话");
taskBindings = await botsService.listBotTaskBindings();
const onTask = taskBindings
  .filter((item) => item.taskId === "task-desktop-1")
  .map((item) => item.botId)
  .sort();
assert.deepEqual(onTask, ["smoke-bot", "smoke-bot-2"], "同一会话可同时绑定多个机器人");
assert.equal(botStreamSubs(), 2, "同一会话上的两个 bot 各自建立流订阅");

// 解绑其中一个：另一个保留（不是"换绑"）
await botsService.unbindBotFromTask({ botId: bot.id, taskId: "task-desktop-1" });
taskBindings = await botsService.listBotTaskBindings();
assert.deepEqual(
  taskBindings.filter((item) => item.taskId === "task-desktop-1").map((item) => item.botId),
  ["smoke-bot-2"],
  "解绑只移除指定 bot",
);

// 工作区解绑同样按 bot 粒度：剩一个仍保留
await botsService.unbindBotFromWorkspace({ botId: bot.id, workspacePath });
assert.deepEqual(
  (await botsService.getWorkspaceBotBinding({ workspacePath })).botIds,
  ["smoke-bot-2"],
);

await botsService.unbindBotFromTask({ botId: otherBot.id, taskId: "task-desktop-1" });
taskBindings = await botsService.listBotTaskBindings();
assert.ok(
  !taskBindings.some((item) => item.taskId === "task-desktop-1"),
  "unbindBotFromTask 后应从绑定列表移除",
);
// ===== 同一个对话只能绑定一个会话：换绑自动切换（并撤掉旧会话的订阅） =====
await botsService.bindBotToTask({
  botId: bot.id,
  conversationKey: "private:smoke-user-1",
  workspacePath,
  taskId: "task-desktop-4",
});
await botsService.bindBotToTask({
  botId: otherBot.id,
  conversationKey: "private:smoke-user-2",
  workspacePath,
  taskId: "task-desktop-4",
});
assert.equal(botStreamSubs(), 2, "同一会话上的两个 bot 各自持有 1 条流订阅");

const switched = await botsService.bindBotToTask({
  botId: bot.id,
  conversationKey: "private:smoke-user-1",
  workspacePath,
  taskId: "task-desktop-5",
});
assert.equal(switched.ok, true, "换绑到另一个会话应成功");
taskBindings = await botsService.listBotTaskBindings();
assert.deepEqual(
  taskBindings.filter((item) => item.botId === bot.id).map((item) => item.taskId),
  ["task-desktop-5"],
  "同一个对话只能绑定一个会话：换绑后旧会话不再出现",
);
assert.equal(
  botStreamSubs(),
  2,
  "换绑必须撤掉旧会话订阅（否则=3：旧会话订阅残留）",
);

// ===== 对话隔离：私聊按用户、群聊按群；一个 bot 可同时服务多个对话（各持一个会话） =====
const multiUserBot = await botsService.saveBot({
  bot: {
    id: "smoke-bot-multi",
    name: "SmokeBotMulti",
    provider: "webhook",
    enabled: true,
    // 全部用户模式：不需要绑定用户，任何私聊用户都能驱动（正是"多私聊各自一个会话"的场景）
    privateChatMode: "all_users",
    allowedWorkspaces: [],
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
    replyMode: "assistant_changes",
  },
});
await botsService.bindBotToWorkspace({ botId: multiUserBot.id, workspacePath });
// 两个私聊用户 → 两个对话 → 各自一个会话（同一个 bot 并发持有两个会话）
await botsService.bindBotToTask({
  botId: multiUserBot.id,
  conversationKey: "private:user-a",
  workspacePath,
  taskId: "task-user-a",
});
await botsService.bindBotToTask({
  botId: multiUserBot.id,
  conversationKey: "private:user-b",
  workspacePath,
  taskId: "task-user-b",
});
const multiStates = (await botsService.getBotStates())
  .filter((item) => item.botId === multiUserBot.id)
  .sort((left, right) => left.conversationKey.localeCompare(right.conversationKey));
assert.deepEqual(
  multiStates.map((item) => [item.conversationKey, item.activeTaskId]),
  [
    ["private:user-a", "task-user-a"],
    ["private:user-b", "task-user-b"],
  ],
  "不同私聊各自持有自己的会话（互不打断）",
);
const multiBindings = (await botsService.listBotTaskBindings()).filter(
  (item) => item.botId === multiUserBot.id,
);
assert.deepEqual(
  multiBindings.map((item) => [item.conversationKey, item.taskId]).sort(),
  [
    ["private:user-a", "task-user-a"],
    ["private:user-b", "task-user-b"],
  ],
  "绑定投影必须逐对话输出（同一个 bot 多条绑定）",
);
const subsBeforeMultiUnbind = botStreamSubs();
assert.equal(subsBeforeMultiUnbind, 4, "两个对话各自一条流订阅（外加前面两个 bot）");
// 按对话解绑：只清指定的那个对话，另一个对话的会话与订阅都不受影响
await botsService.unbindBotFromTask({
  botId: multiUserBot.id,
  taskId: "task-user-a",
  conversationKey: "private:user-a",
});
const multiStatesAfterUnbind = (await botsService.getBotStates()).filter(
  (item) => item.botId === multiUserBot.id,
);
assert.equal(
  multiStatesAfterUnbind.find((item) => item.conversationKey === "private:user-a")?.activeTaskId,
  null,
  "解绑的对话回到草稿",
);
assert.equal(
  multiStatesAfterUnbind.find((item) => item.conversationKey === "private:user-b")?.activeTaskId,
  "task-user-b",
  "其他对话的会话不受影响",
);
assert.equal(botStreamSubs(), subsBeforeMultiUnbind - 1, "只撤掉被解绑对话的订阅");
// 候选对话列表：已存在的对话 + 可预判的绑定用户私聊/名单群
const candidateKeys = (await botsService.listBotConversations({ botId: multiUserBot.id })).map(
  (item) => item.conversationKey,
);
assert.ok(candidateKeys.includes("private:user-a") && candidateKeys.includes("private:user-b"));
console.log("SMOKE OK(+per-conversation sessions: private per user, groups shared, concurrent multi-session)");
// 换绑只影响自己的会话绑定与订阅
const statesAfterSwitch = await botsService.getBotStates();
assert.equal(
  statesAfterSwitch.find((item) => item.botId === "smoke-bot-2").activeTaskId,
  "task-desktop-4",
  "换绑只影响自己的对话",
);

const subsBeforeUnbind = botStreamSubs();
await botsService.unbindBotFromTask({
  botId: bot.id,
  taskId: "task-desktop-5",
  conversationKey: "private:smoke-user-1",
});
assert.equal(subsBeforeUnbind - botStreamSubs(), 1, "解绑后该对话的流订阅应撤掉");
console.log(
  "SMOKE OK(+session bind/mirror/unbind + workspace-eligibility + multi-bot workspace/session + per-conversation session switch)",
);

// ===== 删除机器人 = 自动解绑：会话归属、工作区绑定、运行期流订阅一并消失 =====
const doomedBot = await botsService.saveBot({
  bot: {
    id: "smoke-bot-doomed",
    name: "SmokeBotDoomed",
    provider: "webhook",
    enabled: true,
    providerUserId: "smoke-user-4",
    allowedWorkspaces: [],
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
    replyMode: "assistant_changes",
  },
});
await botsService.bindBotToWorkspace({ botId: doomedBot.id, workspacePath });
await botsService.bindBotToTask({ botId: doomedBot.id, workspacePath, taskId: "task-doomed" });
assert.ok(
  (await botsService.listBotTaskBindings()).some((item) => item.botId === doomedBot.id),
  "删除前应先出现在会话绑定里",
);
const subsBeforeDelete = botStreamSubs();
assert.ok(subsBeforeDelete >= 1, "删除前应有该 bot 的流订阅");

await botsService.deleteBot(doomedBot.id);
const statesAfterDelete = await botsService.getBotStates();
assert.ok(
  !statesAfterDelete.some((item) => item.botId === doomedBot.id),
  "删除后 bot 上下文（会话归属）应被清掉",
);
assert.deepEqual(
  (await botsService.getWorkspaceBotBinding({ workspacePath })).botIds.includes(doomedBot.id),
  false,
  "删除后工作区绑定表不应再引用该 bot",
);
assert.ok(
  !(await botsService.listBotTaskBindings()).some((item) => item.botId === doomedBot.id),
  "删除后会话绑定列表不应再出现该 bot",
);
const subsAfterDelete = botStreamSubs();
assert.equal(subsAfterDelete, subsBeforeDelete - 1, "删除后该 bot 的流订阅应被撤掉");
// 其他 bot（含多对话 bot 仍持有的会话）的订阅不受影响
assert.ok(subsAfterDelete >= 1, "其他 bot 的订阅保留");
console.log("SMOKE OK(+delete-unbinds-bot)");

process.exit(0);
// sqlite 句柄可能未即时释放，尽力清理即可。
try {
  rmSync(dataBaseDir, { recursive: true, force: true });
} catch {
  // Windows 上 EPERM 可忽略：一次性临时目录由系统清理。
}

