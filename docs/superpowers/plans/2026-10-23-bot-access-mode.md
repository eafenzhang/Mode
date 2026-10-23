# Bot 访问控制统一双模式（whitelist / free + 黑名单 + 自动登记）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 落地 spec `bot-access-mode.md`——渠道级 `accessMode`（whitelist/free）统一私聊与群聊准入、用户/群黑名单硬否决、free 模式自动登记，新建渠道一律缺省 whitelist 且存量配置不锁死。

**Architecture:** shared 层新增类型/严格 schema/唯一解析函数 `resolveBotAccessMode`（复用旧 `resolveBotPrivateChatMode` 做存量推导）；services 层在 `handleInboundMessage` 入口与 `withAuthorizedContext` 顶部两处挂黑名单纯函数闸门，free 模式在准入链内原子登记 `userRegistry` / 群 `approved+auto`，新增 6 个 `mutateConfig` 配置操作方法给 UI；UI 新增 `BotAccessModeCard` 并重构私聊卡/群卡按模式分支。配置仍单文件 `bot-config.v3.json`、版本不变。

**Tech Stack:** TypeScript ESM、zod 4 严格 schema、node:test（`tsx --test`）、`@mode/shared` 类型与纯函数。

**Spec:** `docs/specs/bot-access-mode.md`（本计划逐条论据来源；执行者需同时阅读）。本计划执行前，先完成 spec R7 的四个旧 spec 修订（Task 1）。

## Global Constraints

- 新增/修改行为先改 spec；本计划的 spec 已就位，行为分歧一律回改 spec。
- bugfix / 准入分支处写中文注释说明原因与依据（仓库规则）。
- 新文件行数 < 400（architecture policy `maxFileLines`）；`botsService.ts` 已超限，只允许在其中做受控增改，可抽取的纯逻辑必须放新模块。
- i18n 文案 zh-CN 与 en-US 成对新增，缺一不可。
- 日志用既有 `botsLogger`，禁止 `console.log`。
- `botConfigSchema` 是 `.strict()`：BotConfig 每个新增字段必须同步登记到 schema，否则保存抛 ZodError。
- git 提交必须显式给出路径（索引与其他会话共享）；只做本地提交，**禁止 `git push`**。
- 测试命令：services 单文件 `pnpm --filter @mode/services exec tsx --test test/<file>.test.ts`；services 全量 `pnpm --filter @mode/services test`；UI `pnpm --filter @mode/ui test`。
- 验证三件套（Task 7 执行）：`pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`。
- 不读 MyAgents 实现源码（clean-room；行为依据只来自 spec 与 CHANGELOG 描述）。

---

### Task 1: R7 四个旧 spec 修订

**Files:**
- Modify: `docs/specs/im-bot-group-chat-alignment.md`（R3 条款）
- Modify: `docs/specs/im-bot-agent-channel-aplus.md`（私聊规则 2、4）
- Modify: `docs/specs/bot-weixin-open-dm.md`（R1、R5）
- Modify: `docs/specs/im-bot-onboarding.md`（引用微信缺省的段落）

**Interfaces:**
- Consumes: 无（纯文档）。
- Produces: 旧 spec 与 `bot-access-mode.md` 一致的条款，后续任务按修订后条款验收。

- [ ] **Step 1: 修订 im-bot-group-chat-alignment.md R3**

将 R3「『私聊方式』+『群聊激活模式』是唯一准入开关，禁止引入第二套授权模型」改为：

```markdown
R3（2026-10-23 修订，bot-access-mode.md）：`accessMode`（whitelist/free）+ 群激活模式
（disabled/mention/always）是唯一准入开关，禁止引入第二套授权模型；`blockedUsers` /
`blockedGroups` 是对准入结果的纯否决层（只减不加、最先检查），不算第二条授权路径。
```

- [ ] **Step 2: 修订 im-bot-agent-channel-aplus.md 私聊规则 2 与 4**

- 规则 2 的「缺省按 provider 解析：微信 all_users，其余 bound_users」改为「缺省一律
  `whitelist`（新建显式落盘）；存量无 `accessMode` 字段时按 `resolveBotAccessMode`
  推导（微信 free、其余 whitelist），见 bot-access-mode.md R5」。
- 规则 4「微信的差异只在缺省值（all_users）」改为「微信的差异只在存量缺省推导
  （free），新建一律 whitelist；闸门分支四通道完全一致」。

- [ ] **Step 3: 修订 bot-weixin-open-dm.md R1 / R5**

- R1 表格前加一行状态注记：「2026-10-23 起降级为**存量推导规则**：新建微信 Bot 缺省
  改为 `whitelist`（bot-access-mode.md R5）；本表只用于无 `accessMode` 字段的存量配置。」
- R5 的「新建 Bot 时 createDraftBot 显式落盘解析后的缺省值（all_users / bound_users）」
  改为「createDraftBot 显式落盘 `accessMode: "whitelist"`（全渠道）」。

- [ ] **Step 4: 修订 im-bot-onboarding.md 引用段**

检索 `grep -n "all_users\|bot-weixin-open-dm" docs/specs/im-bot-onboarding.md`，把断言
「微信缺省 all_users / 扫码即可用」的段落改为引用 `bot-access-mode.md R5`（存量推导 +
新建 whitelist），保留接入流程本身不变。

- [ ] **Step 5: 提交**

```bash
git add docs/specs/im-bot-group-chat-alignment.md docs/specs/im-bot-agent-channel-aplus.md docs/specs/bot-weixin-open-dm.md docs/specs/im-bot-onboarding.md
git commit -m "docs(specs): R7 修订——准入开关改为 accessMode+黑名单否决，新建缺省 whitelist"
```

---

### Task 2: shared 类型、严格 schema、resolveBotAccessMode

**Files:**
- Modify: `packages/shared/src/bots.ts`（`BotPrivateChatMode` 定义 ~line 245 后；`BotConfig` ~line 248-294；`resolveBotPrivateChatMode` ~line 304；`botConfigSchema` ~line 790-855）
- Test: `packages/services/test/bot-access-mode.test.ts`（新建）

**Interfaces:**
- Produces（后续任务依赖，均从 `@mode/shared` 导出）:
  - `export type BotAccessMode = "whitelist" | "free"`
  - `export interface BotBlockedGroup { groupId: string; groupName?: string; platform: BotProvider; blockedAt: number }`
  - `export interface BotRegisteredUser { providerUserId: string; displayName?: string; discoveredAt: number }`
  - `BotConfig` 新增可选字段：`accessMode?: BotAccessMode`、`blockedUsers?: string[]`、`blockedGroups?: BotBlockedGroup[]`、`userRegistry?: BotRegisteredUser[]`
  - `botConfigSchema` 同步登记上述 4 个字段（`.optional()`，内层对象 `.strict()`）
  - `export function resolveBotAccessMode(bot: Pick<BotConfig, "provider" | "accessMode" | "privateChatMode">): BotAccessMode`

- [ ] **Step 1: 写失败测试**

新建 `packages/services/test/bot-access-mode.test.ts`：

```ts
import assert from "node:assert/strict";
import test from "node:test";

// 访问控制统一双模式（docs/specs/bot-access-mode.md R1/R5）：
// 1) 显式 accessMode 永远优先；
// 2) 无 accessMode 时按存量 privateChatMode 推导（all_users→free、bound_users→whitelist）；
// 3) 两者皆缺省时沿用旧 provider 缺省（微信 free、其余 whitelist）——存量不锁死；
// 4) 严格 schema 必须登记新字段，否则保存抛 ZodError。

test("resolveBotAccessMode：显式 accessMode 优先于 privateChatMode", async () => {
  const { resolveBotAccessMode } = await import("@mode/shared");
  assert.equal(
    resolveBotAccessMode({ provider: "weixin", accessMode: "whitelist", privateChatMode: "all_users" }),
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
  assert.equal(resolveBotAccessMode({ provider: "weixin", privateChatMode: "bound_users" }), "whitelist");
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
        allowedCommands: {},
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
```

- [ ] **Step 2: 运行测试确认失败**

Run: `pnpm --filter @mode/services exec tsx --test test/bot-access-mode.test.ts`
Expected: FAIL——`resolveBotAccessMode is not a function`（或导出缺失）。

- [ ] **Step 3: 实现 shared 类型、schema、解析函数**

`packages/shared/src/bots.ts`：

```ts
/** 渠道访问模式（docs/specs/bot-access-mode.md R1）：
 * whitelist = 绑定/白名单 + 群批准；free = 私聊与群聊免批、自动登记实际使用者。 */
export type BotAccessMode = "whitelist" | "free";

/** 群黑名单：与群授权记录同构但独立存放，准入时最先检查（spec R2 纯否决层）。 */
export interface BotBlockedGroup {
  groupId: string;
  groupName?: string;
  platform: BotProvider;
  blockedAt: number;
}

/** free 模式自动登记的私聊使用者（spec R3）；登记只服务可见与可管理，不是授权路径。 */
export interface BotRegisteredUser {
  providerUserId: string;
  displayName?: string;
  discoveredAt: number;
}
```

`BotConfig` 内（紧跟 `privateChatMode` 字段之后）：

```ts
  /**
   * 渠道访问模式；缺省解析唯一入口 `resolveBotAccessMode`。
   * 新建渠道一律显式落盘 "whitelist"（spec R5）；存量无字段时由旧 privateChatMode / provider 缺省推导。
   */
  accessMode?: BotAccessMode;
  /** 被拉黑的平台用户 id；两种模式下最先检查，命中即静默拒绝（spec R2）。 */
  blockedUsers?: string[];
  /** 被拉黑的群；同上。 */
  blockedGroups?: BotBlockedGroup[];
  /** free 模式自动登记的实际私聊使用者（spec R3）。 */
  userRegistry?: BotRegisteredUser[];
```

`resolveBotPrivateChatMode` 之后新增：

```ts
/**
 * 访问模式唯一解析入口（docs/specs/bot-access-mode.md R1/R5）。
 * 优先级：显式 accessMode → 存量 privateChatMode（all_users→free、bound_users→whitelist）
 * → 旧 provider 缺省（resolveBotPrivateChatMode 内含：微信 free、其余 whitelist）。
 * 存量推导保证「已在用 free 的微信配置」升级后不被锁死；新建渠道由 createDraftBot 显式写 whitelist。
 */
export function resolveBotAccessMode(
  bot: Pick<BotConfig, "provider" | "accessMode" | "privateChatMode">,
): BotAccessMode {
  if (bot.accessMode) {
    return bot.accessMode;
  }
  return resolveBotPrivateChatMode(bot) === "all_users" ? "free" : "whitelist";
}
```

`botConfigSchema`（**strict，必须登记**；放在 `privateChatMode` 行后）：

```ts
    accessMode: z.enum(["whitelist", "free"]).optional(),
    blockedUsers: z.array(z.string().min(1)).optional(),
    blockedGroups: z
      .array(
        z
          .object({
            groupId: z.string().min(1),
            groupName: z.string().optional(),
            platform: z.enum(botProviders),
            blockedAt: z.number(),
          })
          .strict(),
      )
      .optional(),
    userRegistry: z
      .array(
        z
          .object({
            providerUserId: z.string().min(1),
            displayName: z.string().optional(),
            discoveredAt: z.number(),
          })
          .strict(),
      )
      .optional(),
```

- [ ] **Step 4: 运行测试确认通过**

Run: `pnpm --filter @mode/services exec tsx --test test/bot-access-mode.test.ts`
Expected: PASS（4 个用例全绿）。

- [ ] **Step 5: 提交**

```bash
git add packages/shared/src/bots.ts packages/services/test/bot-access-mode.test.ts
git commit -m "feat(bots): shared 新增 accessMode/黑名单/userRegistry 类型与严格 schema"
```

---

### Task 3: botConfigHelpers——findBoundUser 切换 + 黑名单纯函数 + normalize 回填

**Files:**
- Modify: `packages/services/src/bots/botConfigHelpers.ts`（import ~line 1-11；`findBoundUser` ~line 51-63；`normalizeBotConfig` ~line 65-81）
- Modify: `packages/services/test/bot-access-mode.test.ts`（追加用例）

**Interfaces:**
- Consumes: Task 2 的 `resolveBotAccessMode`、`BotBlockedGroup`、`BotRegisteredUser`。
- Produces（Task 4/5 与计划②依赖）:
  - `findBoundUser(bot, actor)` 内部改用 `resolveBotAccessMode(bot) === "free"`（签名不变）
  - `export function isUserBlocked(bot: BotConfig, providerUserId: string): boolean`
  - `export function isGroupBlocked(bot: BotConfig, groupId: string): boolean`
  - `normalizeBotConfig(bot)` 回填 `accessMode: resolveBotAccessMode(bot)`（读取期解析，随下次写盘持久化）

- [ ] **Step 1: 写失败测试**

`bot-access-mode.test.ts` 追加：

```ts
test("findBoundUser：free 模式任何私聊用户放行，whitelist 按绑定收紧", async () => {
  const { findBoundUser } = await import("../src/bots/botConfigHelpers.js");
  const free = {
    id: "b1", provider: "wecom", providerUserId: "owner", allowedUsers: ["t1"],
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
    id: "b1", provider: "wecom",
    blockedUsers: ["u-black"],
    blockedGroups: [{ groupId: "g-black", platform: "wecom", blockedAt: 1 }],
  } as never;
  assert.equal(isUserBlocked(bot, "u-black"), true);
  assert.equal(isUserBlocked(bot, "u-ok"), false);
  assert.equal(isGroupBlocked(bot, "g-black"), true);
  assert.equal(isGroupBlocked(bot, "g-ok"), false);
  assert.equal(isUserBlocked({ id: "b1", provider: "wecom" } as never, "u-any"), false, "无字段视为未拉黑");
});

test("normalizeBotConfig：回填 accessMode（存量推导不锁死，显式值原样保留）", async () => {
  const { normalizeBotConfig } = await import("../src/bots/botConfigHelpers.js");
  const base = {
    id: "b1", name: "b", provider: "weixin", enabled: true,
    allowedWorkspaces: ["*"], allowedCommands: {}, currentOptions: {}, replyMode: "assistant_changes",
  } as never;
  assert.equal(normalizeBotConfig(base).accessMode, "free", "存量微信无字段推导 free");
  assert.equal(
    normalizeBotConfig({ ...base, accessMode: "whitelist" } as never).accessMode,
    "whitelist",
    "显式值原样保留",
  );
  assert.equal(
    normalizeBotConfig({ ...base, provider: "wecom", privateChatMode: "all_users" } as never).accessMode,
    "free",
    "存量 all_users 推导 free",
  );
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `pnpm --filter @mode/services exec tsx --test test/bot-access-mode.test.ts`
Expected: 新增 3 用例 FAIL（`isUserBlocked` 未导出、`findBoundUser` 仍看 `privateChatMode === "all_users"` 导致 `accessMode: "free"` 不生效、`normalizeBotConfig` 无回填）。

- [ ] **Step 3: 实现**

`botConfigHelpers.ts`：

1. import 增加 `resolveBotAccessMode`、`BotAccessMode`（从 `@mode/shared`）。
2. `findBoundUser` 中间分支改为：

```ts
  // free 模式任何私聊用户可驱动（原 all_users 语义，已切换到统一 accessMode，spec R1）。
  if (actor.chatType === "private" && resolveBotAccessMode(bot) === "free") {
    return bot;
  }
```

3. 文件末尾新增：

```ts
/** 私聊黑名单命中（spec bot-access-mode R2）：纯否决层，无字段视为未拉黑。 */
export function isUserBlocked(bot: BotConfig, providerUserId: string): boolean {
  return bot.blockedUsers?.includes(providerUserId) ?? false;
}

/** 群黑名单命中（spec bot-access-mode R2）；groupId 为空串不命中。 */
export function isGroupBlocked(bot: BotConfig, groupId: string): boolean {
  if (!groupId) {
    return false;
  }
  return bot.blockedGroups?.some((item) => item.groupId === groupId) ?? false;
}
```

4. `normalizeBotConfig` 的 `normalized` 对象里追加一行（放在 `replyMode` 行后）：

```ts
    // 读取期回填访问模式：存量推导结果随下次写盘持久化（spec R5），显式值原样保留。
    accessMode: resolveBotAccessMode(bot),
```

- [ ] **Step 4: 运行测试确认通过**

Run: `pnpm --filter @mode/services exec tsx --test test/bot-access-mode.test.ts`
Expected: PASS（Task 2 + Task 3 全部用例）。

- [ ] **Step 5: 回归既有私聊准入测试**

Run: `pnpm --filter @mode/services exec tsx --test test/bot-private-admission.test.ts`
Expected: PASS（旧断言基于 privateChatMode，推导链保留后语义不变）。若有失败，按新语义更新断言并在注释中引用 spec R1。

- [ ] **Step 6: 提交**

```bash
git add packages/services/src/bots/botConfigHelpers.ts packages/services/test/bot-access-mode.test.ts
git commit -m "feat(bots): 准入纯函数切换 accessMode，新增黑名单判定与读取期回填"
```

---

### Task 4: botsService 准入闸门——黑名单双入口 + free 自动登记 + 主动投递切换

**Files:**
- Modify: `packages/services/src/bots/botsService.ts`（import 区 ~line 57；`resolveBotProactiveUserId` ~line 1843；`handleInboundMessage` ~line 7970；`withAuthorizedContext` ~line 6162-6257）
- Test: `packages/services/test/bot-access-mode.test.ts`（追加服务级用例，harness 仿 `test/bot-group-permission-service.test.ts`）

**Interfaces:**
- Consumes: Task 3 的 `findBoundUser`、`isUserBlocked`、`isGroupBlocked`、`resolveBotAccessMode`；既有 `repo.mutateConfig`、`enqueueInboundProcessing`、`groupHistory`。
- Produces（计划②依赖的行为）:
  - 黑名单用户/群的入站在命令解析与激活之前静默返回 `[]`（不登记、不写群历史、不建对话、无回复）
  - free 模式：私聊首条通过准入的消息登记 `userRegistry`；群首条消息把权限记录登记/升级为 `status:"approved", addedBy:"auto"`
  - `resolveBotProactiveUserId` 的 all_users 分支切换为 `resolveBotAccessMode(bot) === "free"`
  - whitelist 模式群 pending→批准流程与私聊绑定流程**行为不变**

- [ ] **Step 1: 写失败测试**

`bot-access-mode.test.ts` 追加服务级用例（harness 头部复制 `bot-group-permission-service.test.ts` 的 `MODE_DATA_BASE_DIR` 模式：`mkdtempSync` → 设 env → 写 `bot-config.v3.json` → `node.createLocalServices` → 取 `IBotsService`）：

```ts
// —— 以下用例共享一个 test 文件内的 harness 工厂 ——
async function withServices(configBots: Record<string, unknown>[]) {
  const dataDir = mkdtempSync(join(tmpdir(), "mode-bot-access-"));
  process.env.MODE_DATA_BASE_DIR = dataDir;
  process.env.MODE_DESKTOP_HOME_DIR = dataDir;
  const configDir = node.getAppConfigDir();
  mkdirSync(configDir, { recursive: true });
  writeFileSync(
    join(configDir, "bot-config.v3.json"),
    `${JSON.stringify({ version: 3, bots: configBots }, null, 2)}\n`,
  );
  const host = await node.createLocalServices({
    modeBuiltinProviderConfigFilePath: join(repoRoot, "config/provider/mode-builtin.json"),
  });
  return { botsService: host.get(services.IBotsService), configDir, dataDir };
}

function baseBot(overrides: Record<string, unknown>) {
  return {
    id: "bot-acc",
    name: "bot-acc",
    provider: "wecom",
    enabled: true,
    allowedWorkspaces: ["*"],
    allowedCommands: {
      status: true, new: true, workspace: true, model: true, mode: true,
      thoughtLevel: true, reply: true,
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
      provider: "wecom", botId: "bot-acc",
      providerUserId: "u1", chatType: "private", providerMessageId: "m-acc",
      ...overrides,
    },
  } as never;
}

test("黑名单：私聊用户在两种模式下都静默拒绝，且不登记 userRegistry", async () => {
  for (const accessMode of ["whitelist", "free"] as const) {
    const { botsService } = await withServices([
      baseBot({ accessMode, blockedUsers: ["u-black"] }),
    ]);
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
  const { botsService } = await withServices([
    baseBot({
      accessMode: "whitelist",
      blockedGroups: [{ groupId: "g-live", platform: "wecom", blockedAt: 1 }],
      groupChat: {
        activation: "always",
        permissions: [{ groupId: "g-live", groupName: "Live", platform: "wecom", status: "approved", discoveredAt: 1 }],
      },
    }),
  ]);
  assert.deepEqual(
    await botsService.handleInboundMessage(
      inbound({ chatType: "group", chatId: "g-live", providerUserId: "u1" }),
    ),
    [],
  );
});

test("free 模式：私聊首条通过准入的消息自动登记 userRegistry，第二条不重复", async () => {
  const { botsService } = await withServices([baseBot({ accessMode: "free" })]);
  await botsService.handleInboundMessage(inbound({ providerUserId: "u-new", displayName: "Alice" }));
  await botsService.handleInboundMessage(inbound({ providerUserId: "u-new", providerMessageId: "m2" }));
  const bot = (await botsService.getConfig()).bots.find((b) => b.id === "bot-acc");
  assert.equal(bot?.userRegistry?.length, 1, "同一用户只登记一次");
  assert.equal(bot?.userRegistry?.[0]?.providerUserId, "u-new");
  assert.equal(bot?.userRegistry?.[0]?.displayName, "Alice");
});

test("free 模式：群首条消息免批登记 approved+auto，直接进入触发链路", async () => {
  const { botsService } = await withServices([
    baseBot({ accessMode: "free", groupChat: { activation: "always" } }),
  ]);
  // 无工作区 → 停在 noWorkspaceAllowed 回复；非空回复即证明没有被 pending 闸门静默。
  const replies = await botsService.handleInboundMessage(
    inbound({ chatType: "group", chatId: "g-free", providerUserId: "u1" }),
  );
  assert.ok(replies.length > 0, "free 群免批必须进入触发链路");
  const permission = (await botsService.getConfig()).bots
    .find((b) => b.id === "bot-acc")?.groupChat?.permissions?.find((p) => p.groupId === "g-free");
  assert.equal(permission?.status, "approved");
  assert.equal(permission?.addedBy, "auto");
});

test("whitelist 模式：群发现仍登记 pending 并静默（A+ 行为不回退）", async () => {
  const { botsService } = await withServices([
    baseBot({ accessMode: "whitelist", groupChat: { activation: "always" } }),
  ]);
  const replies = await botsService.handleInboundMessage(
    inbound({ chatType: "group", chatId: "g-wl", providerUserId: "stranger" }),
  );
  assert.deepEqual(replies, [], "whitelist 未批准群必须静默");
  const permission = (await botsService.getConfig()).bots
    .find((b) => b.id === "bot-acc")?.groupChat?.permissions?.find((p) => p.groupId === "g-wl");
  assert.equal(permission?.status, "pending");
});
```

注：`withServices` 里每次用例重建目录与服务；若 `createLocalServices` 不支持同进程多次创建，退化为「一个 test 内串行多轮写配置 + 单服务」并改用 `saveBot`/直接改写配置文件后重读——以 `bot-group-permission-service.test.ts` 的实际可运行模式为准（该文件证明同进程可创建服务且 `handleInboundMessage` 生效）。dispose 写法以该文件为准。

- [ ] **Step 2: 运行测试确认失败**

Run: `pnpm --filter @mode/services exec tsx --test test/bot-access-mode.test.ts`
Expected: 新增服务级用例 FAIL——黑名单不拦截、free 不登记、群仍 pending。

- [ ] **Step 3: 实现黑名单双入口闸门**

`botsService.ts`：

1. import 增加 `resolveBotAccessMode`（@mode/shared）与 `isUserBlocked`、`isGroupBlocked`（`./botConfigHelpers.js`）。
2. `handleInboundMessage` 的 `enqueueInboundProcessing` 回调**第一段**（elicitation 分支之前）插入：

```ts
      // 黑名单硬否决必须先于命令解析、微信激活与 elicitation 处理（spec bot-access-mode R2）：
      // 被拉黑发送者的消息不得产生任何副作用——不登记 pending、不写群历史、不触发激活欢迎。
      const gateConfig = await repo.readConfig();
      const gateBot = findAuthorizedBot(gateConfig, message.actor);
      if (gateBot) {
        const blocked =
          message.actor.chatType === "group"
            ? Boolean(message.actor.chatId) && isGroupBlocked(gateBot, message.actor.chatId!)
            : isUserBlocked(gateBot, message.actor.providerUserId);
        if (blocked) {
          return [];
        }
      }
```

3. `withAuthorizedContext` 中 `findAuthorizedBot` 与 `bot.id !== message.botId` 校验之后、`chatType !== "private"` 分支之前插入（用该函数已读出的 `config`/`bot`，无额外 IO）：

```ts
    // 黑名单第二道闸门：覆盖不经 handleInboundMessage 的入口（心跳、队列出队重投），
    // 与入口闸门共用纯函数，语义一致（spec R2）。
    if (bot) {
      const blocked =
        message.actor.chatType === "group"
          ? Boolean(message.actor.chatId) && isGroupBlocked(bot, message.actor.chatId)
          : isUserBlocked(bot, message.actor.providerUserId);
      if (blocked) {
        return { ok: false, reply: [] };
      }
    }
```

- [ ] **Step 4: 实现 free 模式自动登记**

4. `withAuthorizedContext` 顶部把

```ts
    const privateChatAllUsers = bot !== null && resolveBotPrivateChatMode(bot) === "all_users";
```

改为

```ts
    // 统一双模式（spec bot-access-mode R1）：free 取代旧 all_users 语义。
    const accessMode = bot ? resolveBotAccessMode(bot) : "whitelist";
    const freeMode = accessMode === "free";
```

（`resolveBotPrivateChatMode` 在本文件剩余引用点见 Step 6 统一替换。）

5. 群分支：在 `const approved = permissions.some(...)` 之后、`isAllowedSender` 之前插入 free 模式登记/升级：

```ts
      // free 模式群免批（spec R3）：首次出现登记 approved+auto；whitelist 时代遗留的
      // pending 在 free 下首条消息同样升级为 approved（addedBy 记 auto 以供 UI 识别来源）。
      if (freeMode && !approved) {
        await repo.mutateConfig((nextConfig) => {
          const target = nextConfig.bots.find((candidate) => candidate.id === bot.id);
          if (!target) return;
          const current = target.groupChat?.permissions ?? [];
          const existing = current.find((item) => item.groupId === chatId);
          target.groupChat = {
            activation: target.groupChat?.activation ?? activation,
            ...target.groupChat,
            permissions: existing
              ? current.map((item) =>
                  item.groupId === chatId
                    ? { ...item, status: "approved" as const, addedBy: "auto" }
                    : item,
                )
              : [
                  ...current,
                  {
                    groupId: chatId,
                    groupName: chatId,
                    platform: bot.provider,
                    status: "approved" as const,
                    discoveredAt: Date.now(),
                    addedBy: "auto",
                  },
                ],
          };
        });
      }
```

随后把 `isAllowedSender` 的首项 `privateChatAllUsers ||` 替换为 `freeMode ||`
（free 下上面已确保 approved；此处保留仅为分支语义完整）。
`if (!approved && !isAllowedSender)` 的 pending 登记分支只在 whitelist 触发——在该分支前加
`if (!freeMode)` 守卫，避免 free 模式落入 pending：

```ts
      if (!approved && !isAllowedSender && !freeMode) {
        // （原 pending 登记 + 群历史 + 静默 return 逻辑原样保留——whitelist 独有，A+ 群规则 5）
        ...
      }
```

6. 私聊分支：`if (!user)` 返回之后插入 free 模式登记：

```ts
    // free 模式自动登记实际使用者（spec R3）：只登记通过准入的用户；
    // 锁内 RMW 幂等，重复消息不重复登记。登记不是授权路径，黑名单已在上方拦截。
    if (freeMode && message.actor.chatType === "private") {
      const registryId = message.actor.providerUserId;
      const alreadyRegistered = bot.userRegistry?.some((item) => item.providerUserId === registryId);
      if (!alreadyRegistered) {
        await repo.mutateConfig((nextConfig) => {
          const target = nextConfig.bots.find((candidate) => candidate.id === bot.id);
          if (!target) return;
          const registry = target.userRegistry ?? [];
          if (registry.some((item) => item.providerUserId === registryId)) return;
          target.userRegistry = [
            ...registry,
            {
              providerUserId: registryId,
              ...(message.actor.displayName ? { displayName: message.actor.displayName } : {}),
              discoveredAt: Date.now(),
            },
          ];
        });
      }
    }
```

- [ ] **Step 5: 主动投递目标切换**

`resolveBotProactiveUserId`（~line 1843）把 `resolveBotPrivateChatMode(bot) === "all_users"`
替换为 `resolveBotAccessMode(bot) === "free"`，注释改为「free（原 all_users）」。

- [ ] **Step 6: 清理本文件旧引用**

Run: `grep -n "resolveBotPrivateChatMode" packages/services/src/bots/botsService.ts`
除 `resolveBotAccessMode` 内部实现（在 shared）外，服务内引用应为 0；若有残留调用点，
语义等价地替换为 `resolveBotAccessMode(...) === "free"` 并保留原注释含义。

- [ ] **Step 7: 运行测试确认通过 + 回归**

Run: `pnpm --filter @mode/services exec tsx --test test/bot-access-mode.test.ts`
Expected: PASS（纯函数 + 服务级全绿）。
Run: `pnpm --filter @mode/services exec tsx --test test/bot-group-permission-service.test.ts`
Expected: PASS（whitelist 群授权流程不回退；若该文件未设 accessMode，存量推导为 whitelist——原语义保留）。
Run: `pnpm --filter @mode/services exec tsx --test test/bot-private-admission.test.ts`
Expected: PASS。

- [ ] **Step 8: 提交**

```bash
git add packages/services/src/bots/botsService.ts packages/services/test/bot-access-mode.test.ts
git commit -m "feat(bots): 入站黑名单双闸门与 free 模式自动登记（accessMode 统一准入）"
```

---

### Task 5: botsService 配置操作方法（6 个）+ 接口

**Files:**
- Modify: `packages/services/src/bots/bots.ts`（`IBotsService` 接口，`approveBotGroup` ~line 254 附近追加）
- Modify: `packages/services/src/bots/botsService.ts`（实现区，`approveBotGroup` ~line 7899 附近追加）
- Test: `packages/services/test/bot-access-mode.test.ts`（追加）

**Interfaces:**
- Produces（Task 6 UI 依赖；签名固定如下；全部走 `repo.mutateConfig` 锁内 RMW、返回更新后的 `BotConfig`）:
  - `setBotAccessMode(params: { botId: string; accessMode: BotAccessMode }): Promise<BotConfig>`
  - `blockBotUser(params: { botId: string; providerUserId: string }): Promise<BotConfig>`
  - `unblockBotUser(params: { botId: string; providerUserId: string }): Promise<BotConfig>`
  - `blockBotGroup(params: { botId: string; groupId: string; groupName?: string }): Promise<BotConfig>`（`platform` 取自 bot 记录，`blockedAt: Date.now()`）
  - `unblockBotGroup(params: { botId: string; groupId: string }): Promise<BotConfig>`
  - `removeBotRegisteredUser(params: { botId: string; providerUserId: string }): Promise<BotConfig>`

- [ ] **Step 1: 写失败测试**

```ts
test("配置操作：mode 切换、拉黑/解除、登记移除均为锁内 RMW 且互不串扰", async () => {
  const { botsService, configDir } = await withServices([
    baseBot({
      accessMode: "whitelist",
      blockedUsers: ["u-x"],
      userRegistry: [{ providerUserId: "u-keep", discoveredAt: 1 }],
      groupChat: {
        activation: "always",
        permissions: [{ groupId: "g-1", groupName: "G1", platform: "wecom", status: "approved", discoveredAt: 1 }],
      },
    }),
  ]);
  let bot = await botsService.setBotAccessMode({ botId: "bot-acc", accessMode: "free" });
  assert.equal(bot.accessMode, "free");

  bot = await botsService.blockBotUser({ botId: "bot-acc", providerUserId: "u-new" });
  assert.deepEqual(bot.blockedUsers, ["u-x", "u-new"], "追加不覆盖既有黑名单");

  bot = await botsService.unblockBotUser({ botId: "bot-acc", providerUserId: "u-x" });
  assert.deepEqual(bot.blockedUsers, ["u-new"]);

  bot = await botsService.blockBotGroup({ botId: "bot-acc", groupId: "g-1", groupName: "G1" });
  assert.equal(bot.blockedGroups?.[0]?.groupId, "g-1");
  assert.equal(bot.blockedGroups?.[0]?.platform, "wecom");

  bot = await botsService.unblockBotGroup({ botId: "bot-acc", groupId: "g-1" });
  assert.equal(bot.blockedGroups?.length, 0);

  bot = await botsService.removeBotRegisteredUser({ botId: "bot-acc", providerUserId: "u-keep" });
  assert.equal(bot.userRegistry?.length, 0);

  // 全部落盘
  const persisted = JSON.parse(readFileSync(join(configDir, "bot-config.v3.json"), "utf8")) as {
    bots: Array<Record<string, unknown>>;
  };
  assert.equal(persisted.bots[0]?.accessMode, "free");
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `pnpm --filter @mode/services exec tsx --test test/bot-access-mode.test.ts`
Expected: FAIL——`botsService.setBotAccessMode is not a function`。

- [ ] **Step 3: 接口声明**

`packages/services/src/bots/bots.ts`（`approveBotGroup` 声明旁追加，doc 注释引用 spec R2/R3/R6）：

```ts
  /** 切换渠道访问模式（spec bot-access-mode R1）。 */
  setBotAccessMode(params: { botId: string; accessMode: BotAccessMode }): Promise<BotConfig>;
  /** 拉黑/解除拉黑私聊用户（spec R2；纯否决层，不改动授权字段）。 */
  blockBotUser(params: { botId: string; providerUserId: string }): Promise<BotConfig>;
  unblockBotUser(params: { botId: string; providerUserId: string }): Promise<BotConfig>;
  /** 拉黑/解除拉黑群（spec R2）。 */
  blockBotGroup(params: { botId: string; groupId: string; groupName?: string }): Promise<BotConfig>;
  unblockBotGroup(params: { botId: string; groupId: string }): Promise<BotConfig>;
  /** 移除 free 模式自动登记的用户（spec R3；移除≠拉黑，下条消息会重新登记）。 */
  removeBotRegisteredUser(params: { botId: string; providerUserId: string }): Promise<BotConfig>;
```

（`BotAccessMode` 需从 `@mode/shared` import 到 `bots.ts`。）

- [ ] **Step 4: 实现**

`botsService.ts` 实现区追加。**共用一个私有辅助**避免复制粘贴（放在 `approveBotGroup` 实现旁）：

```ts
  /** 配置字段的锁内 RMW：找不到 bot 时抛错，否则在 mutateConfig 内原子修改后返回该 bot 记录。 */
  async function mutateBotConfigField(
    botId: string,
    mutate: (bot: BotConfig) => void,
  ): Promise<BotConfig> {
    let saved: BotConfig | undefined;
    await repo.mutateConfig((nextConfig) => {
      const target = nextConfig.bots.find((candidate) => candidate.id === botId);
      if (!target) {
        throw new Error(`Bot not found: ${botId}`);
      }
      mutate(target);
      saved = target;
    });
    if (!saved) throw new Error(`Bot not found: ${botId}`);
    return saved;
  }
```

六个方法各自一行 `mutate`，例如：

```ts
    async setBotAccessMode(params) {
      return mutateBotConfigField(params.botId, (bot) => {
        bot.accessMode = params.accessMode;
      });
    },
    async blockBotUser(params) {
      return mutateBotConfigField(params.botId, (bot) => {
        const list = bot.blockedUsers ?? [];
        if (!list.includes(params.providerUserId)) {
          bot.blockedUsers = [...list, params.providerUserId];
        }
      });
    },
    async unblockBotUser(params) {
      return mutateBotConfigField(params.botId, (bot) => {
        bot.blockedUsers = (bot.blockedUsers ?? []).filter((id) => id !== params.providerUserId);
      });
    },
    async blockBotGroup(params) {
      return mutateBotConfigField(params.botId, (bot) => {
        const list = bot.blockedGroups ?? [];
        if (list.some((item) => item.groupId === params.groupId)) return;
        bot.blockedGroups = [
          ...list,
          {
            groupId: params.groupId,
            ...(params.groupName ? { groupName: params.groupName } : {}),
            platform: bot.provider,
            blockedAt: Date.now(),
          },
        ];
      });
    },
    async unblockBotGroup(params) {
      return mutateBotConfigField(params.botId, (bot) => {
        bot.blockedGroups = (bot.blockedGroups ?? []).filter((item) => item.groupId !== params.groupId);
      });
    },
    async removeBotRegisteredUser(params) {
      return mutateBotConfigField(params.botId, (bot) => {
        bot.userRegistry = (bot.userRegistry ?? []).filter(
          (item) => item.providerUserId !== params.providerUserId,
        );
      });
    },
```

- [ ] **Step 5: 运行测试确认通过**

Run: `pnpm --filter @mode/services exec tsx --test test/bot-access-mode.test.ts`
Expected: PASS。

- [ ] **Step 6: 提交**

```bash
git add packages/services/src/bots/bots.ts packages/services/src/bots/botsService.ts packages/services/test/bot-access-mode.test.ts
git commit -m "feat(bots): 访问模式/黑名单/登记移除的锁内配置操作方法"
```

---

### Task 6: UI——BotAccessModeCard、私聊卡与群卡按模式重构、createDraftBot、i18n

**Files:**
- Create: `packages/ui/src/BotsDialog/BotAccessModeCard.tsx`
- Modify: `packages/ui/src/BotsDialog.tsx`（import 区；`createDraftBot` ~line 87-104；卡片装配 ~line 1696-1745）
- Modify: `packages/ui/src/BotsDialog/BotPrivateChatCard.tsx`（整卡重构，~161 行）
- Modify: `packages/ui/src/BotsDialog/BotGroupChatCard.tsx`（模式分支，~line 50、121-209）
- Modify: `packages/ui/src/i18n/locales/zh-CN.ts`、`packages/ui/src/i18n/locales/en-US.ts`（`bots.groupChat.permissions.*` 键区 ~zh:816 / en 对应区）
- Test: `packages/ui/test/botWeixinOpenDm.test.ts`（按新契约更新）
- Test: `packages/ui/test/botAccessMode.test.ts`（新建，源码契约）

**Interfaces:**
- Consumes: Task 2 `resolveBotAccessMode`；Task 5 六个服务方法（经 `useServices().botsService`）；既有 `onPatchBot`/`saveBot` 模式。
- Produces:
  - `BotAccessModeCard({ bot, onPatchBot, onSetAccessMode, onBlockUser, onUnblockUser, onBlockGroup, onUnblockGroup })`——模式下拉 + 已拉黑用户列表（解除）+ 已拉黑群列表（解除）
  - `createDraftBot` 显式写 `accessMode: "whitelist"`（不再写 `privateChatMode`）
  - 私聊卡：whitelist → 绑定用户 + 绑定码（现状）；free → `userRegistry` 列表（移除、拉黑）
  - 群卡：whitelist → 现有 pending/approved 审批流；free → 无审批按钮，已登记群列表（移除、拉黑）；微信仍整卡隐藏
  - i18n 键：`bots.accessMode.*`、`bots.registry.*`、`bots.groupChat.registered` 系列（zh/en 成对）

- [ ] **Step 1: i18n 文案成对新增**

`zh-CN.ts`（`bots.groupChat.permissions.*` 键区旁）与 `en-US.ts` 对应区各新增：

```ts
  "bots.accessMode.title": "访问模式",
  "bots.accessMode.description": "白名单：仅绑定用户与已批准群可驱动；自由：任何私聊与所在群自动可用并登记。",
  "bots.accessMode.mode.whitelist": "白名单",
  "bots.accessMode.mode.free": "自由",
  "bots.accessMode.blockedUsers": "已拉黑用户",
  "bots.accessMode.blockedGroups": "已拉黑群聊",
  "bots.accessMode.unblockUser": "解除拉黑",
  "bots.accessMode.unblockGroup": "解除拉黑",
  "bots.accessMode.empty": "暂无拉黑记录。",
  "bots.registry.title": "自动登记的使用者",
  "bots.registry.empty": "暂无登记；自由模式下首条私聊消息会自动登记。",
  "bots.registry.remove": "移除",
  "bots.registry.block": "拉黑",
  "bots.registry.blockedBadge": "已拉黑",
  "bots.groupChat.registered": "已登记群（自由模式免批）",
  "bots.groupChat.block": "拉黑",
```

en-US 对应值：`Access mode` / `Whitelist: only bound users and approved groups…` 等（逐键意译，保持同一键名集合；禁止只加一侧）。

- [ ] **Step 2: 写失败的 UI 契约测试**

新建 `packages/ui/test/botAccessMode.test.ts`（模式仿 `botWeixinOpenDm.test.ts` 的源码契约）：

```ts
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

// 访问控制统一双模式（docs/specs/bot-access-mode.md R5/R6）：
// 新建渠道显式落盘 whitelist；模式与黑名单由统一入口解析/操作，UI 不手写缺省。

const read = (relativePath: string): string => readFileSync(new URL(relativePath, import.meta.url), "utf8");

test("createDraftBot 显式落盘 accessMode whitelist（全渠道，含微信）", () => {
  const source = read("../src/BotsDialog.tsx");
  assert.match(source, /accessMode:\s*"whitelist"/, "createDraftBot 必须写 accessMode");
  assert.ok(!source.includes("privateChatMode: resolveBotPrivateChatMode"), "不得再写 privateChatMode 缺省");
});

test("访问模式卡按 resolveBotAccessMode 取值，操作全部经 props 上抛", () => {
  const source = read("../src/BotsDialog/BotAccessModeCard.tsx");
  assert.match(source, /resolveBotAccessMode\(bot\)/);
  assert.match(source, /onSetAccessMode/);
  assert.match(source, /onUnblockUser/);
  assert.match(source, /onUnblockGroup/);
  // 卡片不直接依赖服务实例：服务方法接线在 BotsDialog.tsx
  const dialog = read("../src/BotsDialog.tsx");
  assert.match(dialog, /setBotAccessMode/);
  assert.match(dialog, /unblockBotUser/);
  assert.match(dialog, /unblockBotGroup/);
});

test("私聊卡按 accessMode 分支：free 展示 userRegistry，whitelist 保留绑定码", () => {
  const source = read("../src/BotsDialog/BotPrivateChatCard.tsx");
  assert.match(source, /resolveBotAccessMode\(bot\)/);
  assert.match(source, /userRegistry/);
  assert.ok(!source.includes('privateChatMode ?? "bound_users"'), "不得保留本地缺省值");
});

test("群卡 free 模式无审批按钮", () => {
  const source = read("../src/BotsDialog/BotGroupChatCard.tsx");
  assert.match(source, /resolveBotAccessMode\(bot\)/);
  assert.match(source, /accessMode === "free"/, "free 分支必须显式存在");
});
```

同时**更新** `botWeixinOpenDm.test.ts`（其断言的旧契约被 R7 修订取代）：

- 第一个用例的两个文件改断言 `resolveBotAccessMode(bot)`（替换 `resolveBotPrivateChatMode(bot)`）。
- 第二个用例改断言 `/accessMode:\s*"whitelist"/`。
- 文件头注释补一行：「2026-10-23 修订：缺省统一 whitelist，见 docs/specs/bot-access-mode.md R5。」

- [ ] **Step 3: 运行测试确认失败**

Run: `pnpm --filter @mode/ui test`
Expected: 新用例 FAIL（卡片不存在、createDraftBot 未写 accessMode）。

- [ ] **Step 4: 实现 BotAccessModeCard**

`packages/ui/src/BotsDialog/BotAccessModeCard.tsx`（新文件，<400 行）。结构：

- `SettingsRow` 标题 `bots.accessMode.title`，control 为 `Select`（`value={resolveBotAccessMode(bot)}`，选项 whitelist/free），`onValueChange` 调 `onSetAccessMode`。
- description 按当前模式选 `bots.accessMode.description`（或拆成两条键，任选，zh/en 成对）。
- 已拉黑用户区：`bot.blockedUsers ?? []` 逐行 id + `bots.accessMode.unblockUser` 按钮 → `onUnblockUser(id)`。
- 已拉黑群区：`bot.blockedGroups ?? []` 逐行 `groupName || groupId` + 解除按钮 → `onUnblockGroup(groupId)`；**微信（`resolveBotGroupChatCapabilities(bot.provider)` 无群能力）隐藏群段**。
- 两区空态 `bots.accessMode.empty`。复用 `SettingsRow`、`Select`、`Button` 既有适配器，样式对齐 `BotPrivateChatCard` 的列表行（`flex items-center justify-between ... text-ui-base`）。
- props 签名（固定，BotsDialog 装配依赖）：

```tsx
export function BotAccessModeCard({
  bot,
  onSetAccessMode,
  onUnblockUser,
  onUnblockGroup,
}: {
  bot: BotConfig;
  onSetAccessMode: (accessMode: BotAccessMode) => void;
  onUnblockUser: (userId: string) => void;
  onUnblockGroup: (groupId: string) => void;
}) { ... }
```

- [ ] **Step 5: 重构 BotPrivateChatCard**

- import 换 `resolveBotAccessMode` / `BotAccessMode`（删除 `resolveBotPrivateChatMode`、`BotPrivateChatMode`）。
- `const mode: BotAccessMode = resolveBotAccessMode(bot); const isWhitelist = mode === "whitelist";`
- **删除模式下拉**（移到 BotAccessModeCard）。
- `isBoundUsersMode` 分支改用 `isWhitelist`（绑定用户列表、绑定码面板原样保留）。
- 新增 free 分支：`bot.userRegistry ?? []` 列表——`displayName（若有）+ providerUserId`，两按钮：`bots.registry.remove` → `onPatchBot({ userRegistry: 过滤 })`… 注：登记移除必须走服务方法，故 props 增加 `onRemoveRegistryUser: (userId: string) => void`；拉黑按钮 `bots.registry.block` → `onBlockUser(userId)`。已在黑名单的用户在登记列表中显示 `bots.registry.blockedBadge`（用 `bot.blockedUsers?.includes(id)` 判断）。空态 `bots.registry.empty`。

- [ ] **Step 6: 重构 BotGroupChatCard 模式分支**

- import 换 `resolveBotAccessMode`；`const freeMode = resolveBotAccessMode(bot) === "free";`（`resolveBotPrivateChatMode` 删除；原 `privateAllUsers` 描述文案选择逻辑改由 `freeMode` 驱动，键名沿用 `bots.groupChat.descriptionAllUsers*` 或换成 `bots.accessMode.description`，保持 zh/en 有键）。
- `!freeMode` 时渲染现有 pending（approve/reject）与 approved（remove）列表——原样。
- `freeMode` 时：**不渲染 pending 审批区与 approve/reject 按钮**；渲染统一「已登记群」区（`permissions` 全量，含 pending 遗留），每行两按钮：`bots.groupChat.block` → `onBlockGroup(groupId, groupName)`（新增 prop）、`bots.groupChat.permissions.remove` → 既有 `onRemoveGroup`。标题用 `bots.groupChat.registered`。
- 空态文案键按模式选择（free 用 `bots.registry.empty` 语义的新键 `bots.groupChat.registeredEmpty`——若 Step 1 未加则在此补键，zh/en 成对）。

- [ ] **Step 7: BotsDialog 装配与 createDraftBot**

`BotsDialog.tsx`：

1. `createDraftBot` 返回对象里把

```ts
    // 显式落盘 provider 缺省值：微信 all_users（扫码即可用），其余 bound_users。
    privateChatMode: resolveBotPrivateChatMode({ provider: params.provider }),
```

替换为

```ts
    // 新建一律显式落盘白名单缺省（全渠道含微信，spec bot-access-mode R5）；
    // 存量配置的旧缺省推导由 resolveBotAccessMode 在读取期处理。
    accessMode: "whitelist",
```

（`resolveBotPrivateChatMode` import 若无其他引用则删除；`resolveBotAccessMode` 无需在本文件使用。）

2. 卡片装配：在 `<BotPrivateChatCard ...>` **之前**插入：

```tsx
                  <BotAccessModeCard
                    bot={selectedBot}
                    onSetAccessMode={(accessMode) => {
                      void (async () => {
                        const updated = await botsService.setBotAccessMode({
                          botId: selectedBot.id,
                          accessMode,
                        });
                        setConfig((previous) => ({
                          ...previous,
                          bots: previous.bots.map((bot) => (bot.id === updated.id ? updated : bot)),
                        }));
                      })();
                    }}
                    onUnblockUser={(userId) => { void patchFromService(() => botsService.unblockBotUser({ botId: selectedBot.id, providerUserId: userId })); }}
                    onUnblockGroup={(groupId) => { void patchFromService(() => botsService.unblockBotGroup({ botId: selectedBot.id, groupId })); }}
                  />
```

其中 `patchFromService` 为本文件内新加的小 helper（`const patchFromService = async (op: () => Promise<BotConfig>) => { const updated = await op(); setConfig(...同上 map 替换...); }`），并把 Task 5 相关回调统一改走它以消除重复：

3. `BotPrivateChatCard` 装配处新增两个 props：

```tsx
                    onRemoveRegistryUser={(userId) => { void patchFromService(() => botsService.removeBotRegisteredUser({ botId: selectedBot.id, providerUserId: userId })); }}
                    onBlockUser={(userId) => { void patchFromService(() => botsService.blockBotUser({ botId: selectedBot.id, providerUserId: userId })); }}
```

4. `BotGroupChatCard` 装配处新增：

```tsx
                    onBlockGroup={(groupId, groupName) => { void patchFromService(() => botsService.blockBotGroup({ botId: selectedBot.id, groupId, groupName })); }}
```

（群卡既有 `onApproveGroup/onRejectBotGroup` 三个回调保持不动，可选择顺手改为 `patchFromService`——可选，不强制。）

- [ ] **Step 8: 运行 UI 测试确认通过**

Run: `pnpm --filter @mode/ui test`
Expected: PASS——含更新后的 `botWeixinOpenDm.test.ts` 与新 `botAccessMode.test.ts`，以及既有 `botsDialogLayout.test.ts` / `botsAgentOwnership.test.ts` 等无回归（若 layout 测试断言卡片顺序/文案，按新装配微调断言并在注释引用 spec R6）。

- [ ] **Step 9: 提交**

```bash
git add packages/ui/src/BotsDialog/BotAccessModeCard.tsx packages/ui/src/BotsDialog.tsx packages/ui/src/BotsDialog/BotPrivateChatCard.tsx packages/ui/src/BotsDialog/BotGroupChatCard.tsx packages/ui/src/i18n/locales/zh-CN.ts packages/ui/src/i18n/locales/en-US.ts packages/ui/test/botWeixinOpenDm.test.ts packages/ui/test/botAccessMode.test.ts
git commit -m "feat(ui): 访问模式卡与黑名单管理，私聊/群卡按 whitelist-free 分支"
```

---

### Task 7: 全量验证

**Files:**
- 无新改动；执行验证并修复暴露的问题（修复各自随所在层提交）。

**Interfaces:**
- Consumes: Task 1-6 全部产物。

- [ ] **Step 1: 类型检查**

Run: `pnpm typecheck`
Expected: 0 错误。失败 → 修复（新字段漏登记 / import 缺失为最常见原因）后重跑。

- [ ] **Step 2: Lint**

Run: `pnpm lint`
Expected: 0 错误。失败 → `pnpm lint:fix` 后人工复核剩余项。

- [ ] **Step 3: 架构检查**

Run: `pnpm architecture:check --changed`
Expected: 通过；若新文件被标记（如 UI 层直接引用了 services 实现细节），按提示把依赖收敛到既有入口（`useServices` / `@mode/shared`）。

- [ ] **Step 4: 测试全量**

Run: `pnpm --filter @mode/services test`
Expected: 全绿（`bot-access-mode`、`bot-private-admission`、`bot-group-permission-service`、`bot-config-schema`、`bot-delivery-mode` 等）。
Run: `pnpm --filter @mode/ui test`
Expected: 全绿。

- [ ] **Step 5: 收尾提交（如有修复）**

```bash
git add <修复涉及的显式路径>
git commit -m "fix(bots): 访问模式落地的验证修复（typecheck/lint/测试）"
```

## 非目标（沿用 spec）

- 不删除 `/bind`、绑定码、`allowedUsers`、群批准流程。
- 不做跨 bot / 平台级封禁；不做 v4 配置迁移。
- 不改群激活模式语义；不引入 MyAgents 的入口重排。
