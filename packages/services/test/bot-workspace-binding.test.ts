import assert from "node:assert/strict";
import test from "node:test";
import { normalizeBotWorkspaceBindings } from "@mode/shared";

// 工作区 → bot[] 绑定表：追加/移除的纯函数语义、「绑定 key 指向的工作区
// 已从已知列表消失时自愈为未绑定」、以及 v1 单 bot 字符串的兼容归一化。
// 绑定是 workspace→bot 单向钉定 + bot 可被多工作区绑定（上下文跟随 UI 焦点）的基础。

test("一个工作区可以绑定多个 bot；解绑只影响指定的那个", async () => {
  const { getWorkspaceBoundBots, setWorkspaceBinding, removeWorkspaceBinding } = await import(
    "../src/bots/botsBinding.js"
  );
  let bindings = setWorkspaceBinding(undefined, "/repo-a", "bot-1");
  bindings = setWorkspaceBinding(bindings, "/repo-a", "bot-2");
  assert.deepEqual(getWorkspaceBoundBots(bindings, "/repo-a"), ["bot-1", "bot-2"]);

  // 幂等：重复绑定同一个 bot 不产生重复项，也不换新对象
  assert.equal(setWorkspaceBinding(bindings, "/repo-a", "bot-2"), bindings);

  bindings = setWorkspaceBinding(bindings, "/repo-b", "bot-1");
  assert.deepEqual(getWorkspaceBoundBots(bindings, "/repo-b"), ["bot-1"]);

  // 解绑 bot-2 后 repo-a 仍保留 bot-1
  bindings = removeWorkspaceBinding(bindings, "/repo-a", "bot-2");
  assert.deepEqual(getWorkspaceBoundBots(bindings, "/repo-a"), ["bot-1"]);

  // 解绑最后一个 bot：整条记录删除
  bindings = removeWorkspaceBinding(bindings, "/repo-a", "bot-1");
  assert.equal("/repo-a" in bindings, false);
  assert.deepEqual(removeWorkspaceBinding(bindings, "/missing", "bot-1"), bindings);
  assert.deepEqual(getWorkspaceBoundBots(bindings, "/repo-a"), []);
});

test("resolveBoundWorkspaceRefs 只返回已知工作区（自愈失效绑定）", async () => {
  const { resolveBoundWorkspaceRefs, setWorkspaceBinding } = await import(
    "../src/bots/botsBinding.js"
  );
  const bindings = setWorkspaceBinding(
    setWorkspaceBinding(
      setWorkspaceBinding(undefined, "identity-1", "bot-1"),
      "identity-1",
      "bot-2",
    ),
    "/stale/path",
    "bot-1",
  );
  const refs = [
    {
      id: "identity-1",
      label: "repo-a",
      workspacePath: "/local/repo-a",
      workspaceIdentity: "identity-1",
    },
  ];
  const resolved = resolveBoundWorkspaceRefs(bindings, "bot-1", refs);
  assert.equal(resolved.length, 1);
  assert.equal(resolved[0].id, "identity-1");
  assert.equal(resolved[0].workspacePath, "/local/repo-a");
  // 同一个工作区的另一个 bot 也解析到它自己的绑定
  assert.equal(resolveBoundWorkspaceRefs(bindings, "bot-2", refs).length, 1);

  // 未绑定的 bot：不返回
  assert.equal(resolveBoundWorkspaceRefs(bindings, "bot-3", refs).length, 0);
  assert.equal(resolveBoundWorkspaceRefs(undefined, "bot-1", refs).length, 0);
});

test("一个 bot 只能绑一个工作区：removeBotFromOtherWorkspaces 只保留目标工作区", async () => {
  const { removeBotFromOtherWorkspaces, setWorkspaceBinding, getWorkspaceBoundBots } = await import(
    "../src/bots/botsBinding.js"
  );
  const bindings = setWorkspaceBinding(
    setWorkspaceBinding(setWorkspaceBinding(undefined, "/ws-a", "bot-1"), "/ws-b", "bot-1"),
    "/ws-b",
    "bot-2",
  );
  const next = removeBotFromOtherWorkspaces(bindings, "/ws-b", "bot-1");
  assert.deepEqual(getWorkspaceBoundBots(next, "/ws-a"), []);
  assert.deepEqual(getWorkspaceBoundBots(next, "/ws-b").sort(), ["bot-1", "bot-2"]);
  // 目标工作区之外的其他 bot 不受影响：bot-2 只从 /ws-b 摘掉，/ws-a 的 bot-1 保留
  const forBot2 = removeBotFromOtherWorkspaces(bindings, "/ws-a", "bot-2");
  assert.deepEqual(getWorkspaceBoundBots(forBot2, "/ws-a"), ["bot-1"]);
  assert.deepEqual(getWorkspaceBoundBots(forBot2, "/ws-b"), ["bot-1"]);
});

test("归一化绑定表：v1 单 bot 字符串读成数组，去空白/去重/丢空项", () => {
  assert.deepEqual(
    normalizeBotWorkspaceBindings({
      "/repo-a": "bot-1",
      "/repo-b": ["bot-1", "bot-2", "bot-1", "  "],
      "  ": "bot-3",
      "/repo-c": [],
      "/repo-d": "  bot-4  ",
    }),
    {
      "/repo-a": ["bot-1"],
      "/repo-b": ["bot-1", "bot-2"],
      "/repo-d": ["bot-4"],
    },
  );
  assert.deepEqual(normalizeBotWorkspaceBindings(undefined), {});
});

// 源码契约：绑定与访问范围的收敛不变量。
//
// 为什么不写成行为测试：workspace.list / withAuthorizedContext / bindBotToWorkspace
// 都在 createBotsService 闭包内，需要整套 services 图才能驱动（见 bot-conversation-session
// 同类说明）。这三个不变量是「绑定优先 + 界面如实」的承重点，用源码契约低成本钉住：
//   1. 绑定时访问范围收敛（bind/unbind 都调 convergeBotAllowedWorkspaces）；
//   2. /project 菜单过滤到绑定集（与 /workspace.set 的接受范围一致）；
//   3. 越权判断对绑定工作区放行（否则"在别的工作区改访问范围"会让 bot 全量拒收）。
const botsServiceSource = await import("node:fs/promises").then((fs) =>
  fs
    .readFile(new URL("../src/bots/botsService.ts", import.meta.url), "utf8")
    .catch(() => null),
);

function expectWithin(
  source: string,
  anchor: string,
  expected: string,
  windowLines: number,
  description: string,
): void {
  const lines = source.split("\n");
  const start = lines.findIndex((line) => line.includes(anchor));
  assert.ok(start >= 0, `${description}: 未找到锚点 ${anchor}`);
  const window = lines.slice(start, start + windowLines).join("\n");
  assert.ok(window.includes(expected), `${description}: 锚点后 ${windowLines} 行内未找到 ${expected}`);
}

test("绑定即授权：bind 收敛访问范围，越权判断对绑定工作区放行，菜单过滤到绑定集", {
  skip: !botsServiceSource && "无法读取 botsService 源码",
}, () => {
  const source = botsServiceSource;
  if (!source) return;

  // 1. bind/unbind 都调用收敛函数
  expectWithin(source, "async bindBotToWorkspace", "convergeBotAllowedWorkspaces", 30, "绑定收敛");
  expectWithin(source, "async unbindBotFromWorkspace", "convergeBotAllowedWorkspaces", 30, "解绑收敛");
  assert.ok(
    source.includes("function convergeBotAllowedWorkspaces"),
    "缺少 convergeBotAllowedWorkspaces 定义",
  );
  // 绑定集为空时必须提前返回（不能走到 normalizeAllowedWorkspaces——空数组会变成 "*"）。
  const guardLines = source.split("\n");
  const guardIndex = guardLines.findIndex((line) => line.includes("boundKeys.length === 0"));
  assert.ok(guardIndex >= 0, "收敛函数缺少绑定集为空的守卫");
  assert.ok(
    guardLines
      .slice(guardIndex + 1, guardIndex + 3)
      .some((line) => line.trim() === "return;"),
    "绑定集为空时必须直接 return，保持原访问范围",
  );

  // 2. /project 菜单在绑定时使用绑定集
  expectWithin(
    source,
    'case "workspace.list"',
    "resolveBotBoundWorkspaces",
    40,
    "菜单过滤到绑定集",
  );

  // 3. 越权分支在拒绝前检查绑定工作区（守卫位于拒绝语句之前）
  const outOfScopeIndex = source
    .split("\n")
    .findIndex((line) => line.includes('msg(locale, "workspaceOutOfScope")'));
  assert.ok(outOfScopeIndex >= 0, "未找到 workspaceOutOfScope 拒绝分支");
  assert.ok(
    source
      .split("\n")
      .slice(Math.max(0, outOfScopeIndex - 10), outOfScopeIndex)
      .some((line) => line.includes("isBoundWorkspace")),
    "越权拒绝前必须检查绑定工作区并放行",
  );
});

// 回归背景：绑定表曾存放在 AppSettings.botBindingByWorkspace。设置文件有多个写入方
// （tab 持久化、settings-sync、迁移提交），任一持有过期快照的写入都会把绑定整块覆盖成 {}，
// 表现为"刚绑定好，过几秒又变回未绑定"。修复：绑定落到 bot-bindings.v3.json（BotsRepo 独占）。
test("绑定表与设置文件解耦：并发设置写入不会覆盖绑定", async () => {
  const { mkdtempSync, rmSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");

  const dir = mkdtempSync(join(tmpdir(), "zcodium-bindings-isolation-"));
  process.env.MODE_DATA_BASE_DIR = dir;
  process.env.MODE_DATA_BASE_DIR = dir;
  process.env.HOME = dir;
  process.env.USERPROFILE = dir;

  const { BotsRepo } = await import("../src/bots/repo.js");
  const { createSettingService } = await import("../src/setting/settingService.js");

  const repo = new BotsRepo();
  const settingService = createSettingService();

  await repo.writeBindings({
    version: 1,
    bindings: { "/workspace/alpha": "bot-alpha" },
  });

  // 模拟多个设置写入方（含整快照式写入）反复落盘
  for (let index = 0; index < 5; index += 1) {
    await settingService.update({ lastActiveTabIndex: index });
    await settingService.update({ botBindingByWorkspace: {} }); // 旧快照式写入
  }

  const bindings = await repo.readBindings();
  assert.deepEqual(bindings.bindings, { "/workspace/alpha": "bot-alpha" });
  // 读侧统一归一化成数组（v1 字符串兼容）：一个工作区可绑定多个 bot。
  assert.deepEqual(
    normalizeBotWorkspaceBindings(bindings.bindings),
    { "/workspace/alpha": ["bot-alpha"] },
  );

  try {
    rmSync(dir, { recursive: true, force: true });
  } catch {
    // Windows 句柄未释放时忽略。
  }
});

// 源码契约：会话绑定资格（会话只能绑定当前工作区的机器人）+ 多机器人并行。
//
// 行为覆盖属于 smoke（packages/services/scripts/smoke-bot-binding.mjs 里用真实服务图
// 绑定两个 bot 到同一工作区/同一会话）；这里只钉住承重点防止被无声改回去：
//   1. bindBotToTask 先用 isBotEligibleForSessionBinding（shared 谓词，UI 同源）判资格，
//      并以 reason "workspace" 拒绝非本工作区的 bot；
//   2. 工作区绑定是「追加/按 bot 解绑」，不是整体替换；
//   3. 任务流订阅 key 带 botId —— 否则同一会话上的第二个 bot 被当成重复订阅拿不到流事件。
test("多机器人：工作区绑定是追加语义，会话绑定按工作区资格放行，流订阅按 bot 分键", {
  skip: !botsServiceSource && "无法读取 botsService 源码",
}, () => {
  const source = botsServiceSource;
  if (!source) return;

  // 1. 会话绑定资格：先判资格、后补授权，非本工作区 return reason "workspace"
  const bindIndex = source.indexOf("async bindBotToTask");
  assert.ok(bindIndex >= 0, "未找到 bindBotToTask");
  const bindBlock = source.slice(bindIndex, bindIndex + 2400);
  const eligibilityIndex = bindBlock.indexOf("isBotEligibleForSessionBinding");
  assert.ok(eligibilityIndex >= 0, "bindBotToTask 必须做会话绑定资格校验");
  assert.ok(
    bindBlock.includes('reason: "workspace"') &&
      eligibilityIndex < bindBlock.indexOf("isWorkspaceAllowed(workspaceKey"),
    '资格校验必须先于补授权写入，并以 reason "workspace" 拒绝',
  );
  assert.ok(
    bindBlock.includes("getWorkspaceBoundBots(botBindings, workspaceKey)"),
    "资格校验必须用工作区绑定表（bot 列表）判断归属",
  );

  // 2. 追加/按 bot 解绑语义（不能退化成"一个工作区一个 bot"的替换）；
  //    同时一个 bot 只能绑一个工作区：绑定时先把它从其他工作区摘掉。
  expectWithin(source, "async bindBotToWorkspace", "removeBotFromOtherWorkspaces(bindings, workspaceKey, bot.id)", 20, "绑定前摘除旧工作区");
  expectWithin(source, "async bindBotToWorkspace", "setWorkspaceBinding(", 24, "绑定追加");
  expectWithin(
    source,
    "async unbindBotFromWorkspace",
    "removeWorkspaceBinding(bindings, workspaceKey, params.botId)",
    20,
    "按 bot 解绑",
  );
  assert.ok(
    !source.includes("detachTaskFromOtherBots"),
    "多机器人场景下不允许再出现单会话单 bot 的强制摘除逻辑",
  );

  // 3. 流订阅 key 带 botId（同一会话多 bot 各自订阅）
  const detachKeyIndex = source.indexOf("function buildTaskStreamSubscriptionKey");
  assert.ok(detachKeyIndex >= 0, "缺少 buildTaskStreamSubscriptionKey");
  assert.ok(
    source
      .slice(detachKeyIndex, detachKeyIndex + 500)
      .includes("[getWorkspaceKey(workspacePath, workspaceIdentity), taskId, botId].join"),
    "订阅 key 必须包含 botId",
  );
  expectWithin(
    source,
    "async function watchTaskStream",
    "buildTaskStreamSubscriptionKey(",
    20,
    "watch 使用分 bot 的订阅 key",
  );

  // 4. UI 焦点同步：逐对话判断——草稿对话跟随工作区，已绑会话的对话绝不被夺走
  //    （否则绿点会跟着桌面选中的会话跑："绑定的会话只在选中时显示"）。
  const focusIndex = source.indexOf("async function applyUiFocus");
  assert.ok(focusIndex >= 0, "未找到 applyUiFocus");
  const focusBlock = source.slice(focusIndex, focusIndex + 1600);
  assert.ok(
    focusBlock.includes("getWorkspaceBoundBots(bindings, workspaceKey)") &&
      focusBlock.includes("for (const botId of botIds)"),
    "applyUiFocus 必须遍历工作区绑定的全部 bot",
  );
  assert.ok(
    focusBlock.includes("for (const conversation of listConversations(state, botId))") &&
      focusBlock.includes("if (conversation.activeTaskId)") &&
      focusBlock.includes("continue;"),
    "已有会话绑定的对话不能被桌面焦点夺走",
  );
  assert.ok(
    focusBlock.includes("conversation.conversationKey,") &&
      focusBlock.includes("params.workspacePath,") &&
      focusBlock.includes("null,"),
    "草稿对话只跟随工作区（taskId 传 null，不自动绑定会话）",
  );

  // 5. 打字指示器同样按 bot 分键（否则同一会话只有第一个 bot 显示"正在输入"）
  const typingKeyIndex = source.indexOf("function buildTypingKey");
  assert.ok(typingKeyIndex >= 0, "缺少 buildTypingKey");
  assert.ok(
    source.slice(typingKeyIndex, typingKeyIndex + 220).includes("[taskId, botId].join"),
    "打字指示器 key 必须包含 botId",
  );

  // 6. 一个对话只能绑定一个会话：该对话离开旧会话时撤订阅（收口在 writeContext，
  //    /task.set、/new、UI 焦点、bot 新建任务四条路径共用），并广播刷新桌面绑定投影。
  //    其他对话的槽位与订阅不受影响（多对话并发的前提）。
  const writeContextIndex = source.indexOf("async function writeContext");
  assert.ok(writeContextIndex >= 0, "未找到 writeContext");
  const writeBlock = source.slice(writeContextIndex, writeContextIndex + 1600);
  assert.ok(
    writeBlock.includes("previous.activeTaskId !== context.activeTaskId") &&
      writeBlock.includes("disposeTaskStreamSubscription("),
    "writeContext 必须在同对话内切换会话时撤掉旧会话订阅",
  );
  assert.ok(
    writeBlock.includes("stopTyping(previous.activeTaskId, context.botId)"),
    "切换会话时必须停掉旧会话的打字指示",
  );
  const focusBlockForBroadcast = source.slice(
    source.indexOf("async function focusBotOnWorkspace"),
    source.indexOf("async function focusBotOnWorkspace") + 3200,
  );
  assert.ok(
    focusBlockForBroadcast.includes('"active_task_changed"') &&
      focusBlockForBroadcast.includes('source: "ui"'),
    "焦点/换绑后必须广播 active_task_changed(source ui) 刷新绿点与菜单投影",
  );

  // 7. 删除机器人即解绑：清工作区绑定 + 撤运行期订阅 + 广播刷新投影
  const deleteIndex = source.indexOf("async deleteBot(botId: string)");
  assert.ok(deleteIndex >= 0, "未找到 deleteBot");
  const deleteBlock = source.slice(deleteIndex, deleteIndex + 2200);
  assert.ok(
    deleteBlock.includes("disposeBotRuntimeBindings(botId)"),
    "删除时撤掉该 bot 的运行期绑定（流订阅/打字指示/临时卡片）",
  );
  assert.ok(
    deleteBlock.includes("pruneBotFromWorkspaceBindings(botId)"),
    "删除时把它从工作区绑定表里摘掉（不留幽灵机器人）",
  );
  assert.ok(
    deleteBlock.includes('"active_task_changed"') && deleteBlock.includes('source: "ui"'),
    "删除掉会话绑定后要广播一次刷新绿点/标题栏图标",
  );
  assert.ok(
    source.includes("function disposeBotRuntimeBindings") &&
      source.includes("function pruneBotFromWorkspaceBindings"),
    "缺少删除解绑的两个 helper",
  );
});
