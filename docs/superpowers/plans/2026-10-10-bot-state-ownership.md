# Bot 状态锁内原子 RMW Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 消灭 `bot-state.v3.json` 的跨对话/跨窗口丢更新竞态：一切读-改-写收口到锁内原子的 `BotsRepo.mutateState`。

**Architecture:** 文件锁（进程内 FIFO + 跨进程 OS 锁）内的「读最新→mutator→zod 校验→写盘」为唯一写入命令入口；进程内不留状态缓存；副作用（撤订阅/停 typing/发提示）一律在锁外执行。

**Tech Stack:** TypeScript ESM、node:test（`tsx --test`）、`setDataBaseDir` 临时目录隔离。

**Spec:** `docs/specs/bot-state-ownership.md`（执行者需同时阅读）

## Global Constraints

- mutator 内禁止任何 repo 文件操作（同路径嵌套 `withFileLock` 会自锁死锁）——每处转换必须遵守。
- 只读路径（`readState`、`readContext` 的读取）语义不变。
- config 文件（`bot-config.v3.json`）的 RMW 不在本次范围（spec 非目标）。
- 不执行 `git commit` / `git push`（由人发起）。
- 验证命令与 B 相同：`pnpm --filter @mode/services test`、`pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`。

---

### Task 1: `BotsRepo.mutateState` + 并发丢更新回归测试

**Files:**
- Modify: `packages/services/src/bots/repo.ts`（`readState` line 81-98 重构为共享读 + 新增 `mutateState`）
- Test: `packages/services/test/bot-state-mutation.test.ts`（新建）

**Interfaces:**
- Produces（Task 2 依赖）:
  - `async BotsRepo.mutateState(mutator: (state: BotsStateFile) => void | Promise<void>): Promise<BotsStateFile>` —— 锁内读→改→校验→写，返回落盘后的状态；mutator 抛错或校验失败时不写盘、错误原样冒泡

- [ ] **Step 1: 写失败测试**

新建 `packages/services/test/bot-state-mutation.test.ts`：

```ts
import assert from "node:assert/strict";
import { mkdtemp, mkdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import type { BotsStateFile, BotConversationState } from "@mode/shared";
import { setDataBaseDir } from "../src/paths.js";

// 背景：bot-state 曾是 readState→改→writeState 快照覆盖，两次文件锁之间
// 跨对话/跨窗口并发会丢更新（docs/specs/bot-state-ownership.md）。
// 本测试把「锁内原子 RMW」的零丢失承诺固定下来。

async function withTempDataDir(run: (dir: string) => Promise<void>): Promise<void> {
  const dir = await mkdtemp(join(tmpdir(), "mode-bot-state-mutation-"));
  setDataBaseDir(dir);
  try {
    await mkdir(join(dir, ".mode", "v2"), { recursive: true });
    await run(dir);
  } finally {
    setDataBaseDir(null);
    await rm(dir, { recursive: true, force: true });
  }
}

function makeConversation(botId: string, id: string): BotConversationState {
  return {
    botId,
    conversationKey: `private:${id}`,
    conversationKind: "private",
    conversationId: id,
    workspacePath: "C:/workspace",
    mode: "draft",
    activeTaskId: null,
    updatedAt: 1,
  };
}

test("并发 20 个 mutateState 各写各的对话槽位：零丢失", async () => {
  await withTempDataDir(async () => {
    const { BotsRepo } = await import("../src/bots/repo.js");
    const repo = new BotsRepo();
    await Promise.all(
      Array.from({ length: 20 }, (_unused, index) =>
        repo.mutateState((state: BotsStateFile) => {
          const channel = (state.bots["bot-1"] ??= {
            botId: "bot-1",
            conversations: {},
            updatedAt: Date.now(),
          });
          channel.conversations[`private:u${index}`] = makeConversation("bot-1", `u${index}`);
          channel.updatedAt = Date.now();
        }),
      ),
    );
    const final = await repo.readState();
    const conversations = final.bots["bot-1"]?.conversations ?? {};
    assert.equal(Object.keys(conversations).length, 20);
  });
});

test("并发 20 次同槽位追加：全部保留", async () => {
  await withTempDataDir(async () => {
    const { BotsRepo } = await import("../src/bots/repo.js");
    const repo = new BotsRepo();
    await repo.mutateState((state: BotsStateFile) => {
      state.bots["bot-1"] = {
        botId: "bot-1",
        conversations: { "private:u1": makeConversation("bot-1", "u1") },
        updatedAt: Date.now(),
      };
    });
    await Promise.all(
      Array.from({ length: 20 }, (_unused, index) =>
        repo.mutateState((state: BotsStateFile) => {
          const slot = state.bots["bot-1"]!.conversations["private:u1"]!;
          slot.queuedMessages = [
            ...(slot.queuedMessages ?? []),
            { text: `m${index}`, receivedAt: index },
          ];
        }),
      ),
    );
    const final = await repo.readState();
    const queued = final.bots["bot-1"]!.conversations["private:u1"]!.queuedMessages ?? [];
    assert.equal(queued.length, 20);
    assert.deepEqual(
      queued.map((item) => item.text).sort(),
      Array.from({ length: 20 }, (_unused, index) => `m${index}`).sort(),
    );
  });
});

test("mutator 抛错或写入非法值：状态不变且错误冒泡", async () => {
  await withTempDataDir(async () => {
    const { BotsRepo } = await import("../src/bots/repo.js");
    const repo = new BotsRepo();
    await repo.mutateState((state: BotsStateFile) => {
      state.bots["bot-1"] = {
        botId: "bot-1",
        conversations: { "private:u1": makeConversation("bot-1", "u1") },
        updatedAt: Date.now(),
      };
    });
    await assert.rejects(() =>
      repo.mutateState((state: BotsStateFile) => {
        state.bots["bot-1"]!.conversations["private:u1"]!.workspacePath = "";
        throw new Error("boom");
      }),
    );
    // 非法值（workspacePath 空串违反 schema）也必须被拦截且不落盘
    await assert.rejects(() =>
      repo.mutateState((state: BotsStateFile) => {
        state.bots["bot-1"]!.conversations["private:u1"]!.workspacePath = "";
      }),
    );
    const final = await repo.readState();
    assert.equal(final.bots["bot-1"]!.conversations["private:u1"]!.workspacePath, "C:/workspace");
  });
});
```

- [ ] **Step 2: 运行确认失败**

Run: `pnpm --filter @mode/services exec tsx --test test/bot-state-mutation.test.ts`
Expected: FAIL（`repo.mutateState is not a function`）

- [ ] **Step 3: 实现 `mutateState`**

`repo.ts`：把 `readState` 的锁内读逻辑提取为私有方法，`readState` 与 `mutateState` 共用：

```ts
  /** 锁内读（含迁移固化）；仅供 readState / mutateState 在已持锁的临界区里调用。 */
  private async readStateLocked(path: string): Promise<BotsStateFile> {
    const current = await readOptionalJson(path);
    if (current !== undefined) return botsStateFileSchema.parse(current);
    const v2 = await readOptionalJson(join(getAppConfigDir(), BOTS_V2_STATE_FILE));
    const legacy = v2 === undefined ? await readOptionalJson(join(getAppConfigDir(), BOTS_LEGACY_STATE_FILE)) : v2;
    const state = botsStateFileSchema.parse(
      legacy === undefined ? { version: 3, bots: {} } : importLegacyBotState(legacy),
    );
    // 在同一文件锁内固定迁移结果；后续登录/套餐变化不再重新解释旧身份。
    await writeJson(path, state);
    return state;
  }

  async readState(): Promise<BotsStateFile> {
    const path = join(getAppConfigDir(), BOTS_STATE_FILE);
    return withFileLock(path, () => this.readStateLocked(path));
  }

  /**
   * 锁内原子读-改-写：整个「读最新 → mutator → zod 校验 → 写盘」持有同一把
   * 文件锁（进程内 FIFO + 跨进程 OS 锁），消除 readState…writeState 快照覆盖
   * 在两次锁之间被并发写入方打断的丢更新竞态（docs/specs/bot-state-ownership.md）。
   * 不变量：mutator 内禁止再调用 repo 的文件操作——同路径嵌套 withFileLock
   * 会因进程内 FIFO 队列自锁（前一个持锁者就是调用方自己）。
   */
  async mutateState(mutator: (state: BotsStateFile) => void | Promise<void>): Promise<BotsStateFile> {
    const path = join(getAppConfigDir(), BOTS_STATE_FILE);
    return withFileLock(path, async () => {
      const state = await this.readStateLocked(path);
      await mutator(state);
      const parsed = botsStateFileSchema.parse(state);
      await writeJson(path, parsed);
      return parsed;
    });
  }
```

注意：原 `readState` 中「损坏时暴露错误、绝不回退」与迁移写盘语义原样保留（`readStateLocked` 就是原函数体）。`BotsStateFile` 类型校验：`readStateLocked` 返回的 parse 结果 mutator 修改后再 `parse` 一次即为校验。

- [ ] **Step 4: 运行确认通过**

Run: `pnpm --filter @mode/services exec tsx --test test/bot-state-mutation.test.ts`
Expected: PASS（3 个用例）

---

### Task 2: 转换 botsService 全部 RMW 写入点

**Files:**
- Modify: `packages/services/src/bots/botsService.ts`（10 处 `repo.writeState`，按函数名定位）

**Interfaces:**
- Consumes: Task 1 的 `repo.mutateState`

**统一转换规则**（每处适用）：

```ts
// 旧模式（竞态）：
const state = await repo.readState();
<基于快照的修改>;
await repo.writeState(state);

// 新模式：
await repo.mutateState((state) => {
  <同样的修改，只动自己的槽位/字段>;
});
// 需要向外传递的中间结果用外层闭包变量在 mutator 内捕获；
// 副作用（撤订阅/停 typing/发提示/广播）在 await 之后执行。
```

- [ ] **Step 1: 直接转换（机械点，逻辑不变）**

按函数名逐个转换以下写入方，语义逐字保持：

1. `patchBotChannelState`：`ensureBotChannelState(state, botId)` + `Object.assign` 移入 mutator。
2. `queueContextMessage`：入队计算移入 mutator；外层捕获 `EnqueueQueuedMessageResult`；
   通道/槽位不存在的早退分支改为捕获标志位后在外层返回 `{ queue: [], position: 1 }`。
3. `drainQueuedMessages`：把「定位持有该 taskId 的槽位 → mode/running 守卫 → 出队」
   整体移入 mutator 并捕获 `{ context, queued }`；守卫不通过时在外层 return；
   `handleMessage` 重投递保持在锁外（它自身会再入队/再写状态）。
4. `clearDeliveryNoticeIfSet`：读判断 + 清除合并为单个 mutator（不再二次 readState）。
5. `rekeyLegacyConversations`（原 line ~2365-2391）：读→重映射→固化写整体移入
   mutator，注释「无论是否重映射都写一次」语义不变。
6. bot 凭据重置（`repo.writeState` @ 原 6919）与 bot 删除（原 6964）：
   `delete state.bots[botId]` 移入 mutator；删除处 `listConversations(state, botId)`
   的结果在 mutator 内捕获到外层 `removedConversations`，后续 dispose 逻辑在锁外。
7. `resetBotState`：按键删除/整 bot 清空移入 mutator（键解析 `parseBotConversationKey`
   保持在锁外）。

- [ ] **Step 2: 特殊点 `writeContext`（副作用外置）**

```ts
  async function writeContext(context: BotContextState): Promise<void> {
    // 修复原因：快照覆盖写在两次文件锁之间会被其他对话/窗口的写入打断，
    // 用锁内原子 RMW；撤流订阅与停 typing 是副作用，必须挪到锁外执行
    // （docs/specs/bot-state-ownership.md 规则 3）。
    let previous: BotContextState | undefined;
    let disposeTarget:
      | { workspacePath: string; workspaceIdentity: string | undefined; activeTaskId: string }
      | undefined;
    await repo.mutateState((state) => {
      const channel = ensureBotChannelState(state, context.botId);
      const existing = channel.conversations[context.conversationKey];
      previous = existing;
      if (
        existing?.activeTaskId &&
        (existing.activeTaskId !== context.activeTaskId ||
          getWorkspaceKey(existing.workspacePath, existing.workspaceIdentity) !==
            getWorkspaceKey(context.workspacePath, context.workspaceIdentity))
      ) {
        disposeTarget = {
          workspacePath: existing.workspacePath,
          workspaceIdentity: existing.workspaceIdentity,
          activeTaskId: existing.activeTaskId,
        };
      }
      channel.conversations[context.conversationKey] = { ...context, updatedAt: Date.now() };
      channel.updatedAt = Date.now();
    });
    if (disposeTarget) {
      const target = disposeTarget;
      disposeTaskStreamSubscription(
        target.workspacePath,
        target.workspaceIdentity,
        target.activeTaskId,
        context.botId,
      );
      stopTyping(target.activeTaskId, context.botId);
    }
  }
```

（`previous` 变量若无后续读取可省略，仅保留 `disposeTarget`；以实际引用为准。）

- [ ] **Step 3: 特殊点 `parkOutboundMessage`（提示在锁外）**

捕获 `shouldNotify` / `result.dropped` / `code` 到外层；`await repo.mutateState(...)` 之后
依次执行：dropped warn、parked warn、`shouldNotify` 时发送 `deliveryParked` 提示
（提示发送本身在锁外，失败只记日志——既有逻辑不变）。

- [ ] **Step 4: 收尾检查**

Run: `rg -n "repo.writeState\(" packages/services/src/bots/botsService.ts`
Expected: 0 处（repo 内部迁移写除外）

- [ ] **Step 5: 全量测试**

Run: `pnpm --filter @mode/services test`
Expected: PASS（含 bot-state-mutation 3 用例、outbound-delivery 9 用例与全部既有测试；如实报告任何失败）

---

### Task 3: 验证三件套 + 汇报

- [ ] **Step 1:** `pnpm typecheck` → 0 错误
- [ ] **Step 2:** `pnpm lint` → 0 错误（告警与基线对比，只报与本改动相关的）
- [ ] **Step 3:** `pnpm architecture:check --changed` → violations 0（基线 0）
- [ ] **Step 4:** 汇报改动清单、真实结果、spec 验收对应、残余风险（同对话快照替换语义由 per-conversation 串行链守护；UI 焦点写入不在链上——列出该残余项）

## Self-Review 记录

1. **Spec 覆盖**：规则 1→Task 1/2 全部转换；规则 2→不做缓存（无对应任务=明确非目标）；
   规则 3→Task 2 Step 2/3 副作用外置 + 不变量注释；规则 4→转换规则「只动自己槽位」；
   规则 5→只读路径不改；验收 1/3→Task 1 测试 + Task 2 Step 5；验收 2→注释与 Task 3 检查；
   验收 4→Task 3；验收 5（实机）→汇报中标注需人工核对。
2. **占位符**：无 TBD；机械转换给了逐函数清单与统一规则。
3. **类型一致性**：`mutateState` 签名 Task 1 定义、Task 2 消费一致；测试用 `BotsStateFile`/
   `BotConversationState` 与 shared 定义一致。
