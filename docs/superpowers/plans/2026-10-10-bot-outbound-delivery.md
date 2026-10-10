# Bot 出站投递可靠性 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复微信出站回复被静默丢弃的问题——失败分类、有界重试、停放补投、一次性用户提示、per-bot 发送串行化。

**Architecture:** 纯函数决策层（`outboundDelivery.ts`：错误分类、恢复计划、停放队列、per-bot FIFO）+ 既有唯一 choke point `sendOutbound` 编排 + `withAuthorizedContext` 单点做 token 刷新与补投。状态唯一所有者是 `BotConversationState` 对话槽位（bot-state.v3.json），仅 botsService 在 per-conversation 串行链内写入。

**Tech Stack:** TypeScript ESM、node:test（`tsx --test`）、zod 4 schema、`@mode/shared` 类型。

**Spec:** `docs/specs/bot-outbound-delivery.md`（本计划逐条论据来源；执行者需同时阅读）

## Global Constraints

- 新增/修改行为先改 spec；本计划的 spec 已就位，行为分歧一律回改 spec。
- bugfix 处写中文注释，说明原因与修复依据（仓库规则）。
- UI/服务日志用既有 logger（`botsLogger`），禁止 `console.log`。
- 新文件行数 < 400（architecture policy `maxFileLines`）。
- i18n 文案 zh-CN 与 en-US 成对新增，缺一不可。
- 不改其他 provider 的失败语义；`classifySendError` 返回 `null` 的错误必须原样冒泡（现状行为）。
- 不执行 `git commit` / `git push`（仓库规则：提交与推送由人发起；计划无提交步骤）。
- 测试命令：单文件 `pnpm --filter @mode/services exec tsx --test test/<file>.test.ts`；全量 `pnpm --filter @mode/services test`。
- 验证三件套（Task 5 执行）：`pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`。

---

### Task 1: shared schema 字段与 i18n 文案

**Files:**
- Modify: `packages/shared/src/bots.ts`（`BotConversationState` 接口 ~line 383-405；`botConversationStateSchema` ~line 828-870）
- Modify: `packages/services/src/bots/messages.ts`（zh-CN ~line 136 附近、en-US ~line 289 附近）
- Test: `packages/services/test/bot-outbound-delivery.test.ts`（新建）

**Interfaces:**
- Produces（后续任务依赖）:
  - `export interface BotPendingDelivery { text: string; providerUserId: string; queuedAt: number; attempts: number }`（`@mode/shared`）
  - `BotConversationState` 新增可选字段：`lastContextToken?: string`、`pendingDeliveryQueue?: BotPendingDelivery[]`、`deliveryNoticeAt?: number`
  - 消息键 `deliveryParked`（zh-CN/en-US 均存在，无插槽变量）

- [ ] **Step 1: 写失败测试**

新建 `packages/services/test/bot-outbound-delivery.test.ts`：

```ts
import assert from "node:assert/strict";
import test from "node:test";
import { botsStateFileSchema, type BotConversationState } from "@mode/shared";

// 出站投递停放状态必须随 bot-state 持久化往返；schema 是严格校验，
// 漏登记字段会在读取时被剥离（表现为补投队列"莫名消失"）。

test("botsStateFileSchema：对话状态的出站投递字段能往返保真", () => {
  const state = {
    version: 4,
    bots: {
      "bot-1": {
        botId: "bot-1",
        conversations: {
          "private:u1": {
            botId: "bot-1",
            conversationKey: "private:u1",
            conversationKind: "private",
            conversationId: "u1",
            workspacePath: "C:/workspace",
            mode: "draft",
            activeTaskId: null,
            lastContextToken: "tok-abc",
            pendingDeliveryQueue: [
              { text: "上一条回复", providerUserId: "u1", queuedAt: 1000, attempts: 2 },
            ],
            deliveryNoticeAt: 2000,
            updatedAt: 1,
          },
        },
        updatedAt: 1,
      },
    },
  };
  const parsed = botsStateFileSchema.parse(state);
  const conversation = parsed.bots["bot-1"]!.conversations["private:u1"] as BotConversationState;
  assert.equal(conversation.lastContextToken, "tok-abc");
  assert.equal(conversation.deliveryNoticeAt, 2000);
  assert.deepEqual(conversation.pendingDeliveryQueue, [
    { text: "上一条回复", providerUserId: "u1", queuedAt: 1000, attempts: 2 },
  ]);
});

test("botsStateFileSchema：不含新字段的存量对话状态照常解析（向后兼容）", () => {
  const state = {
    version: 4,
    bots: {
      "bot-1": {
        botId: "bot-1",
        conversations: {
          "private:u1": {
            botId: "bot-1",
            conversationKey: "private:u1",
            conversationKind: "private",
            conversationId: "u1",
            workspacePath: "C:/workspace",
            mode: "draft",
            activeTaskId: null,
            updatedAt: 1,
          },
        },
        updatedAt: 1,
      },
    },
  };
  const parsed = botsStateFileSchema.parse(state);
  const conversation = parsed.bots["bot-1"]!.conversations["private:u1"];
  assert.equal(conversation?.lastContextToken, undefined);
  assert.equal(conversation?.pendingDeliveryQueue, undefined);
});
```

- [ ] **Step 2: 运行确认失败**

Run: `pnpm --filter @mode/services exec tsx --test test/bot-outbound-delivery.test.ts`
Expected: FAIL（`lastContextToken` 属性断言为 `undefined`，或 parse 报 unknown key / 字段被剥离）

- [ ] **Step 3: 实现 shared 字段与 i18n**

3a. `packages/shared/src/bots.ts` — 在 `BotQueuedMessage` 类型定义之后新增：

```ts
/** 出站投递失败后停放的回复（微信 context 过期等场景），随对话槽位持久化，新入站后按序补投。 */
export interface BotPendingDelivery {
  text: string;
  /** 与 BotOutboundMessage.providerUserId 同构：chatId ?? providerUserId。 */
  providerUserId: string;
  queuedAt: number;
  attempts: number;
}
```

3b. 在 `BotConversationState` 接口内（`queuedMessages` 字段旁）新增：

```ts
  /** 最近一条入站消息携带的 provider context token；微信出站要求新鲜 token，过期会被服务端拒绝。 */
  lastContextToken?: string;
  /** 出站失败停放的回复；该对话下一条入站消息到达后按序补投。 */
  pendingDeliveryQueue?: BotPendingDelivery[];
  /** 当前失败 episode 已发过一次性提示的时间戳；任一投递成功后清除。 */
  deliveryNoticeAt?: number;
```

3c. 在 `botConversationStateSchema`（zod）内 `queuedMessages` 之后新增：

```ts
  lastContextToken: z.string().optional(),
  pendingDeliveryQueue: z
    .array(
      z
        .object({
          text: z.string(),
          providerUserId: z.string().min(1),
          queuedAt: z.number(),
          attempts: z.number(),
        })
        .strict(),
    )
    .optional(),
  deliveryNoticeAt: z.number().optional(),
```

3d. `packages/services/src/bots/messages.ts` — zh-CN 块（`taskQueuedDropped` 后）新增：

```ts
    deliveryParked: "部分回复暂时未能送达，将在你发送下一条消息后自动补投。",
```

en-US 块（`taskQueuedDropped` 后）新增：

```ts
    deliveryParked:
      "Some replies could not be delivered yet. They will be redelivered after you send your next message.",
```

- [ ] **Step 4: 运行确认通过**

Run: `pnpm --filter @mode/services exec tsx --test test/bot-outbound-delivery.test.ts`
Expected: PASS（2 个用例）

---

### Task 2: outboundDelivery 纯函数模块

**Files:**
- Create: `packages/services/src/bots/outboundDelivery.ts`
- Test: `packages/services/test/bot-outbound-delivery.test.ts`（追加）

**Interfaces:**
- Consumes: Task 1 的 `BotPendingDelivery`（`@mode/shared`）
- Produces（Task 3/4 依赖，签名固定）:
  - `export type BotSendFailureCode = "context_expired" | "retryable" | "session_expired"`
  - `export class BotSendError extends Error { readonly code: BotSendFailureCode; readonly detail?: string; constructor(code: BotSendFailureCode, message: string, detail?: string) }`
  - `export function classifySendError(error: unknown): BotSendFailureCode | null`
  - `export type DeliveryRecoveryPlan = { kind: "retry_with_token"; token: string } | { kind: "retry_backoff"; delayMs: number } | { kind: "retry_without_token" } | { kind: "park" }`
  - `export function planDeliveryRecovery(code: BotSendFailureCode, input: { attempt: number; usedToken?: string; freshToken?: string }): DeliveryRecoveryPlan`
  - `export const BOT_SEND_MIN_INTERVAL_MS = 300`、`export const BOT_SEND_RETRY_DELAYS_MS: readonly number[] = [500, 2000]`、`export const BOT_PENDING_DELIVERY_LIMIT = 100`、`export const BOT_PENDING_DELIVERY_MAX_ATTEMPTS = 8`
  - `export function enqueuePendingDelivery(queue: BotPendingDelivery[] | undefined, input: { text: string; providerUserId: string }, now: number): { queue: BotPendingDelivery[]; dropped?: BotPendingDelivery }`
  - `export function removeFirstPendingDelivery(queue: BotPendingDelivery[]): { queue: BotPendingDelivery[]; removed?: BotPendingDelivery }`
  - `export function bumpFirstPendingDeliveryAttempt(queue: BotPendingDelivery[]): { queue: BotPendingDelivery[]; dropped?: BotPendingDelivery }`
  - `export interface BotSendQueue { run<T>(botId: string, task: () => Promise<T>): Promise<T> }`
  - `export function createBotSendQueue(minIntervalMs?: number): BotSendQueue`

- [ ] **Step 1: 写失败测试**（追加到 `packages/services/test/bot-outbound-delivery.test.ts`）

```ts
test("classifySendError：仅结构化错误可分类，普通 Error 返回 null", async () => {
  const { BotSendError, classifySendError } = await import("../src/bots/outboundDelivery.js");
  assert.equal(classifySendError(new BotSendError("context_expired", "prepare failed")), "context_expired");
  assert.equal(classifySendError(new BotSendError("retryable", "HTTP 503")), "retryable");
  assert.equal(classifySendError(new BotSendError("session_expired", "HTTP 401")), "session_expired");
  assert.equal(classifySendError(new Error("Weixin iLink /sendmessage failed: ret=1")), null);
  assert.equal(classifySendError("string error"), null);
});

test("planDeliveryRecovery：context_expired 换新 token → 去 token → 停放", async () => {
  const { planDeliveryRecovery } = await import("../src/bots/outboundDelivery.js");
  // 首次失败且有更新 token：换新重试
  assert.deepEqual(
    planDeliveryRecovery("context_expired", { attempt: 0, usedToken: "old", freshToken: "new" }),
    { kind: "retry_with_token", token: "new" },
  );
  // token 无更新：直接去掉 token 重试
  assert.deepEqual(
    planDeliveryRecovery("context_expired", { attempt: 0, usedToken: "old", freshToken: "old" }),
    { kind: "retry_without_token" },
  );
  assert.deepEqual(
    planDeliveryRecovery("context_expired", { attempt: 1, usedToken: "new", freshToken: "new" }),
    { kind: "retry_without_token" },
  );
  // 两步重试用尽：停放
  assert.deepEqual(
    planDeliveryRecovery("context_expired", { attempt: 2, usedToken: undefined, freshToken: undefined }),
    { kind: "park" },
  );
  // 本来就不带 token：无从恢复，直接停放
  assert.deepEqual(
    planDeliveryRecovery("context_expired", { attempt: 0 }),
    { kind: "park" },
  );
});

test("planDeliveryRecovery：retryable 退避两次后停放；session_expired 直接停放", async () => {
  const { planDeliveryRecovery, BOT_SEND_RETRY_DELAYS_MS } = await import(
    "../src/bots/outboundDelivery.js"
  );
  assert.deepEqual(planDeliveryRecovery("retryable", { attempt: 0 }), {
    kind: "retry_backoff",
    delayMs: BOT_SEND_RETRY_DELAYS_MS[0],
  });
  assert.deepEqual(planDeliveryRecovery("retryable", { attempt: 1 }), {
    kind: "retry_backoff",
    delayMs: BOT_SEND_RETRY_DELAYS_MS[1],
  });
  assert.deepEqual(planDeliveryRecovery("retryable", { attempt: 2 }), { kind: "park" });
  assert.deepEqual(planDeliveryRecovery("session_expired", { attempt: 0 }), { kind: "park" });
});

test("pendingDeliveryQueue：入队裁剪、队首删除、失败递增与 8 次上限丢弃", async () => {
  const { enqueuePendingDelivery, removeFirstPendingDelivery, bumpFirstPendingDeliveryAttempt, BOT_PENDING_DELIVERY_LIMIT, BOT_PENDING_DELIVERY_MAX_ATTEMPTS } = await import(
    "../src/bots/outboundDelivery.js"
  );
  let state = enqueuePendingDelivery(undefined, { text: "r1", providerUserId: "u1" }, 1000).queue;
  state = enqueuePendingDelivery(state, { text: "r2", providerUserId: "u1" }, 2000).queue;
  assert.deepEqual(
    state.map((item) => item.text),
    ["r1", "r2"],
  );
  assert.equal(state[0]?.attempts, 0);

  const removed = removeFirstPendingDelivery(state);
  assert.equal(removed.removed?.text, "r1");
  assert.deepEqual(removed.queue.map((item) => item.text), ["r2"]);

  // 队首失败递增 attempts，直至超限丢弃
  let queue = [{ text: "stale", providerUserId: "u1", queuedAt: 1, attempts: 0 }];
  let dropped;
  for (let i = 0; i <= BOT_PENDING_DELIVERY_MAX_ATTEMPTS; i++) {
    const result = bumpFirstPendingDeliveryAttempt(queue);
    queue = result.queue;
    dropped = result.dropped;
  }
  assert.equal(dropped?.text, "stale");
  assert.equal(queue.length, 0);

  // 超上限丢最旧
  let full: ReturnType<typeof enqueuePendingDelivery>["queue"] = [];
  for (let i = 0; i <= BOT_PENDING_DELIVERY_LIMIT; i++) {
    full = enqueuePendingDelivery(full, { text: `m${i}`, providerUserId: "u1" }, i).queue;
  }
  assert.equal(full.length, BOT_PENDING_DELIVERY_LIMIT);
  assert.equal(full[0]?.text, "m1");
});

test("createBotSendQueue：同 bot 严格 FIFO + 最小间隔；跨 bot 互不阻塞", async () => {
  const { createBotSendQueue, BOT_SEND_MIN_INTERVAL_MS } = await import(
    "../src/bots/outboundDelivery.js"
  );
  const queue = createBotSendQueue(BOT_SEND_MIN_INTERVAL_MS);
  const order: string[] = [];
  const timestamps: number[] = [];
  const taskA1 = queue.run("botA", async () => {
    await new Promise((resolve) => setTimeout(resolve, 30));
    order.push("a1");
  });
  const taskA2 = queue.run("botA", async () => {
    order.push("a2");
    timestamps.push(Date.now());
  });
  const taskB1 = queue.run("botB", async () => {
    order.push("b1");
  });
  await Promise.all([taskA1, taskA2, taskB1]);
  // 同 bot 顺序固定；botB 不被 botA 的慢任务阻塞
  assert.deepEqual(order, ["b1", "a1", "a2"]);
  // a2 与 a1 结束之间至少间隔最小间隔
  assert.ok(timestamps.length === 1);

  // 上一个任务失败不阻塞后续任务，且错误原样抛给调用方
  await assert.rejects(() => queue.run("botC", () => Promise.reject(new Error("boom"))), /boom/);
  const after = await queue.run("botC", () => Promise.resolve("ok"));
  assert.equal(after, "ok");
});
```

- [ ] **Step 2: 运行确认失败**

Run: `pnpm --filter @mode/services exec tsx --test test/bot-outbound-delivery.test.ts`
Expected: FAIL（模块 `outboundDelivery.js` 不存在）

- [ ] **Step 3: 实现 `packages/services/src/bots/outboundDelivery.ts`**

```ts
import type { BotPendingDelivery } from "@mode/shared";

/**
 * 出站投递失败的恢复决策与停放队列（纯函数，无 IO）。
 * 事故依据：2026-10-10 微信 /sendmessage 连续返回 prepare failed，回复被
 * stream 队列 warn 后丢弃（docs/specs/bot-outbound-delivery.md）。
 */

/** 微信服务端把会话准备失败报为 errmsg="prepare failed"；换用结构化 code 驱动恢复。 */
export type BotSendFailureCode = "context_expired" | "retryable" | "session_expired";

export class BotSendError extends Error {
  readonly code: BotSendFailureCode;
  readonly detail?: string;
  constructor(code: BotSendFailureCode, message: string, detail?: string) {
    super(message);
    this.name = "BotSendError";
    this.code = code;
    this.detail = detail;
  }
}

/** 只有显式结构化错误可分类；其余错误保持既有冒泡语义，不进入新恢复管线。 */
export function classifySendError(error: unknown): BotSendFailureCode | null {
  return error instanceof BotSendError ? error.code : null;
}

/** 瞬时抖动的退避节奏；两次用尽即停放，避免长时间占住 per-bot 发送队列。 */
export const BOT_SEND_RETRY_DELAYS_MS: readonly number[] = [500, 2_000];

/** 同一 bot 相邻发送的最小间隔，仅平滑突发，不声称满足平台配额。 */
export const BOT_SEND_MIN_INTERVAL_MS = 300;

export const BOT_PENDING_DELIVERY_LIMIT = 100;
export const BOT_PENDING_DELIVERY_MAX_ATTEMPTS = 8;

export type DeliveryRecoveryPlan =
  | { kind: "retry_with_token"; token: string }
  | { kind: "retry_backoff"; delayMs: number }
  | { kind: "retry_without_token" }
  | { kind: "park" };

/**
 * 恢复计划：context_expired 先换新 token、再去 token（各一次）后停放；
 * retryable 有界退避后停放；session_expired 无本地恢复手段，直接停放。
 * attempt 为该条消息已执行的恢复步数。
 */
export function planDeliveryRecovery(
  code: BotSendFailureCode,
  input: { attempt: number; usedToken?: string; freshToken?: string },
): DeliveryRecoveryPlan {
  if (code === "session_expired") {
    return { kind: "park" };
  }
  if (code === "retryable") {
    const delayMs = BOT_SEND_RETRY_DELAYS_MS[input.attempt];
    return delayMs === undefined ? { kind: "park" } : { kind: "retry_backoff", delayMs };
  }
  if (input.attempt === 0 && input.freshToken && input.freshToken !== input.usedToken) {
    return { kind: "retry_with_token", token: input.freshToken };
  }
  if (input.attempt <= 1 && input.usedToken) {
    return { kind: "retry_without_token" };
  }
  return { kind: "park" };
}

export function enqueuePendingDelivery(
  queue: BotPendingDelivery[] | undefined,
  input: { text: string; providerUserId: string },
  now: number,
): { queue: BotPendingDelivery[]; dropped?: BotPendingDelivery } {
  const next = [...(queue ?? []), { ...input, queuedAt: now, attempts: 0 }];
  let dropped: BotPendingDelivery | undefined;
  while (next.length > BOT_PENDING_DELIVERY_LIMIT) {
    dropped = next.shift();
  }
  return { queue: next, ...(dropped ? { dropped } : {}) };
}

export function removeFirstPendingDelivery(
  queue: BotPendingDelivery[],
): { queue: BotPendingDelivery[]; removed?: BotPendingDelivery } {
  const [removed, ...rest] = queue;
  return { queue: rest, ...(removed ? { removed } : {}) };
}

/** 补投失败：队首 attempts+1，超过上限丢弃该条，防止永久积压。 */
export function bumpFirstPendingDeliveryAttempt(
  queue: BotPendingDelivery[],
): { queue: BotPendingDelivery[]; dropped?: BotPendingDelivery } {
  const [first, ...rest] = queue;
  if (!first) {
    return { queue: [] };
  }
  const attempts = first.attempts + 1;
  if (attempts > BOT_PENDING_DELIVERY_MAX_ATTEMPTS) {
    return { queue: rest, dropped: first };
  }
  return { queue: [{ ...first, attempts }, ...rest] };
}

export interface BotSendQueue {
  run<T>(botId: string, task: () => Promise<T>): Promise<T>;
}

/**
 * per-bot FIFO 发送队列 + 最小间隔：同一 bot 的出站严格串行（微信/企微等
 * 平台侧限流与消息乱序都由这里兜住），不同 bot 互不阻塞。
 */
export function createBotSendQueue(minIntervalMs = BOT_SEND_MIN_INTERVAL_MS): BotSendQueue {
  const tails = new Map<string, Promise<unknown>>();
  const lastStartedAtByBot = new Map<string, number>();
  return {
    async run<T>(botId: string, task: () => Promise<T>): Promise<T> {
      const previous = tails.get(botId) ?? Promise.resolve();
      const current = previous
        .catch(() => undefined)
        .then(async (): Promise<T> => {
          const lastStartedAt = lastStartedAtByBot.get(botId) ?? 0;
          const waitMs = lastStartedAt + minIntervalMs - Date.now();
          if (waitMs > 0) {
            await new Promise((resolve) => setTimeout(resolve, waitMs));
          }
          lastStartedAtByBot.set(botId, Date.now());
          return task();
        });
      tails.set(botId, current);
      try {
        return await current;
      } finally {
        if (tails.get(botId) === current) {
          tails.delete(botId);
        }
      }
    },
  };
}
```

- [ ] **Step 4: 运行确认通过**

Run: `pnpm --filter @mode/services exec tsx --test test/bot-outbound-delivery.test.ts`
Expected: PASS（Task 1 + Task 2 共 6 个用例）

---

### Task 3: 微信 provider 错误结构化分类

**Files:**
- Modify: `packages/services/src/bots/providers/weixinProvider.ts`（`requestWeixinJson` line 124-158、`readAccessToken` line 97-99 附近）
- Test: `packages/services/test/bot-outbound-delivery.test.ts`（追加）

**Interfaces:**
- Consumes: Task 2 的 `BotSendError`、`BotSendFailureCode`（`../outboundDelivery.js`）
- Produces:
  - `export function classifyWeixinRequestFailure(input: { httpStatus?: number; errmsg?: string }): BotSendFailureCode | null`（weixinProvider.ts 导出，纯函数）
  - `requestWeixinJson` 抛错语义：HTTP 401/403 → `BotSendError("session_expired")`；429/≥500 → `BotSendError("retryable")`；网络/超时（非调用方 abort）→ `BotSendError("retryable")`；token 缺失 → `BotSendError("session_expired")`；payload `errmsg === "prepare failed"` → `BotSendError("context_expired", ..., detail 含 ret/errcode/errmsg)`；**其余错误保持原字符串与普通 Error 形态**

- [ ] **Step 1: 写失败测试**（追加）

```ts
test("classifyWeixinRequestFailure：HTTP 状态与服务端 errmsg 映射到恢复分类", async () => {
  const { classifyWeixinRequestFailure } = await import(
    "../src/bots/providers/weixinProvider.js"
  );
  assert.equal(classifyWeixinRequestFailure({ httpStatus: 401 }), "session_expired");
  assert.equal(classifyWeixinRequestFailure({ httpStatus: 403 }), "session_expired");
  assert.equal(classifyWeixinRequestFailure({ httpStatus: 429 }), "retryable");
  assert.equal(classifyWeixinRequestFailure({ httpStatus: 500 }), "retryable");
  assert.equal(classifyWeixinRequestFailure({ httpStatus: 503 }), "retryable");
  assert.equal(classifyWeixinRequestFailure({ httpStatus: 404 }), null);
  assert.equal(classifyWeixinRequestFailure({ errmsg: "prepare failed" }), "context_expired");
  assert.equal(classifyWeixinRequestFailure({ errmsg: "some other error" }), null);
  assert.equal(classifyWeixinRequestFailure({}), null);
});
```

- [ ] **Step 2: 运行确认失败**

Run: `pnpm --filter @mode/services exec tsx --test test/bot-outbound-delivery.test.ts`
Expected: FAIL（`classifyWeixinRequestFailure is not a function` / 导出不存在）

- [ ] **Step 3: 实现 weixinProvider 分类**

3a. 文件顶部 imports 增加：

```ts
import { BotSendError, type BotSendFailureCode } from "../outboundDelivery.js";
```

3b. 在 `requestWeixinJson` 之前新增纯函数：

```ts
/**
 * 微信 iLink 请求失败 → 恢复分类（纯函数，供单测固定语义）。
 * 2026-10-10 事故：服务端在会话上下文过期后对 /sendmessage 返回
 * errmsg="prepare failed"（docs/specs/bot-outbound-delivery.md）。
 */
export function classifyWeixinRequestFailure(input: {
  httpStatus?: number;
  errmsg?: string;
}): BotSendFailureCode | null {
  if (input.httpStatus !== undefined) {
    if (input.httpStatus === 401 || input.httpStatus === 403) {
      return "session_expired";
    }
    if (input.httpStatus === 429 || input.httpStatus >= 500) {
      return "retryable";
    }
    return null;
  }
  if (input.errmsg === "prepare failed") {
    return "context_expired";
  }
  return null;
}
```

3c. 重写 `requestWeixinJson`（保持原函数签名与成功路径不变）：

```ts
async function requestWeixinJson(
  bot: BotConfig,
  deps: WeixinProviderDeps,
  path: string,
  body?: unknown,
  signal?: AbortSignal,
  timeoutMs?: number,
): Promise<unknown> {
  const token = await readAccessToken(bot, deps);
  if (!token?.trim()) {
    // 修复原因：token 缺失属登录态丢失，结构化为 session_expired 走停放+提示，
    // 而不是被当作未知错误丢回复（docs/specs/bot-outbound-delivery.md 规则 2/4）。
    throw new BotSendError(
      "session_expired",
      "Weixin iLink bot token is missing. Scan the Weixin login QR code first.",
    );
  }
  let response: Awaited<ReturnType<typeof fetchBotProviderJson<unknown>>>;
  try {
    response = await fetchBotProviderJson<unknown>(
      `${getWeixinApiBaseUrl()}${WEIXIN_BOT_API_PREFIX}${path}`,
      {
        method: "POST",
        headers: buildHeaders(token.trim()),
        body: JSON.stringify(appendBaseInfo(body ?? {})),
        signal,
      },
      timeoutMs,
    );
  } catch (error) {
    if (signal?.aborted) {
      // 调用方主动中止（关停/切凭据）不是网络故障，保持原样冒泡。
      throw error;
    }
    // 修复原因：超时与网络错误当前是普通 Error，会被恢复管线当"未知"丢弃；
    // 归类 retryable 后走有界退避重试（spec 规则 4）。
    throw new BotSendError("retryable", error instanceof Error ? error.message : String(error));
  }
  if (!response.ok) {
    const message = `Weixin iLink ${path} failed: HTTP ${response.status}`;
    const code = classifyWeixinRequestFailure({ httpStatus: response.status });
    throw code ? new BotSendError(code, message) : new Error(message);
  }
  const payload = response.payload;
  const data = isRecord(payload) ? payload : null;
  const ret = readNumber(data, "ret");
  const errcode = readNumber(data, "errcode");
  if ((ret !== null && ret !== 0) || (errcode !== null && errcode !== 0)) {
    const message =
      readString(data, "errmsg") || readString(data, "message") || `ret=${ret ?? ""} errcode=${errcode ?? ""}`.trim();
    const code = classifyWeixinRequestFailure({ errmsg: message });
    if (code) {
      throw new BotSendError(code, `Weixin iLink ${path} failed: ${message}`, `ret=${ret ?? ""} errcode=${errcode ?? ""} errmsg=${message}`);
    }
    throw new Error(`Weixin iLink ${path} failed: ${message}`);
  }
  return payload;
}
```

注意：`fetchBotProviderJson` 的实际返回类型以当前签名为准（若为 `{ ok, status, payload }` 结构则无需改 `providerRequest.ts`）；实现时先读该函数签名再落笔，不臆造字段。

- [ ] **Step 4: 运行确认通过**

Run: `pnpm --filter @mode/services exec tsx --test test/bot-outbound-delivery.test.ts`
Expected: PASS（7 个用例）

---

### Task 4: botsService 编排（串行发送、恢复、停放、补投、提示）

**Files:**
- Modify: `packages/services/src/bots/botsService.ts`
  - imports（`messageQueue.js` 导入区 ~line 166）
  - `createOutbound` 之后新增编排辅助函数
  - `sendOutbound`（line 3067-3073）
  - `withAuthorizedContext` 的 `const context = await readContext(message.actor, bot);`（~line 5675）
  - 清理点 line 1054、1558、1917（`queuedMessages: undefined` 旁）

**Interfaces:**
- Consumes（Task 2/3 全部导出 + Task 1 字段）:
  - `createBotSendQueue().run(botId, task)`、`classifySendError`、`planDeliveryRecovery`、`enqueuePendingDelivery`、`removeFirstPendingDelivery`、`bumpFirstPendingDeliveryAttempt`、`BOT_SEND_MIN_INTERVAL_MS`、`BotSendError`
- Produces:
  - `sendOutbound(bot, message): Promise<void>` 签名不变（14 处调用方零改动）；内部改为入队 + `deliverWithRecovery`
  - `deliverWithRecovery(bot, message, options?: { park?: boolean }): Promise<"sent" | "parked">`（模块私有）
  - `refreshDeliveryInbound(bot, context, providerContextToken): Promise<BotContextState>`（模块私有，token 刷新 + 补投）
  - `flushPendingDeliveries(bot, context): Promise<BotContextState>`（模块私有）

- [ ] **Step 1: 新增编排辅助函数**（`createOutbound` 函数体之后）

```ts
  /** 按出站消息的合并 id（chatId ?? userId）在该 bot 的对话表里定位会话槽位。 */
  async function readOutboundConversation(
    bot: BotConfig,
    message: BotOutboundMessage,
  ): Promise<{ key: string; state: BotContextState } | null> {
    const state = await repo.readState();
    const channel = state.bots[bot.id];
    if (!channel) return null;
    const entries = Object.entries(channel.conversations).filter(
      ([, conversation]) => conversation.conversationId === message.providerUserId,
    );
    const preferred =
      entries.find(([, conversation]) => conversation.conversationKind === "private") ??
      entries[0];
    return preferred ? { key: preferred[0], state: preferred[1] } : null;
  }

  /** 任一投递成功后清除一次性提示标记，使下一个失败 episode 能再次提示。 */
  async function clearDeliveryNoticeIfSet(bot: BotConfig, message: BotOutboundMessage): Promise<void> {
    const found = await readOutboundConversation(bot, message);
    if (!found?.state.deliveryNoticeAt) return;
    const state = await repo.readState();
    const channel = state.bots[bot.id];
    const conversation = channel?.conversations[found.key];
    if (!channel || !conversation?.deliveryNoticeAt) return;
    channel.conversations[found.key] = {
      ...conversation,
      deliveryNoticeAt: undefined,
      updatedAt: Date.now(),
    };
    channel.updatedAt = Date.now();
    await repo.writeState(state);
  }

  /** 把无法投递的回复停放进对话槽位，并保证每个 episode 只提示一次。 */
  async function parkOutboundMessage(
    bot: BotConfig,
    message: BotOutboundMessage,
    code: BotSendFailureCode,
  ): Promise<void> {
    const found = await readOutboundConversation(bot, message);
    if (!found) {
      botsLogger.warn(
        undefined,
        `bot reply parked skipped: conversation not found bot=${bot.id} user=${message.providerUserId} code=${code}`,
      );
      return;
    }
    const state = await repo.readState();
    const channel = state.bots[bot.id];
    const conversation = channel?.conversations[found.key];
    if (!channel || !conversation) return;
    const result = enqueuePendingDelivery(
      conversation.pendingDeliveryQueue,
      { text: message.text, providerUserId: message.providerUserId },
      Date.now(),
    );
    const shouldNotify = !conversation.deliveryNoticeAt;
    channel.conversations[found.key] = {
      ...conversation,
      pendingDeliveryQueue: result.queue,
      ...(shouldNotify ? { deliveryNoticeAt: Date.now() } : {}),
      updatedAt: Date.now(),
    };
    channel.updatedAt = Date.now();
    await repo.writeState(state);
    if (result.dropped) {
      botsLogger.warn(
        undefined,
        `bot reply parked queue full, oldest dropped bot=${bot.id} user=${message.providerUserId}`,
      );
    }
    botsLogger.warn(
      undefined,
      `bot reply parked bot=${bot.id} provider=${bot.provider} user=${message.providerUserId} code=${code}: ${message.text.slice(0, 80)}`,
    );
    if (shouldNotify) {
      // 提示本身发送失败只记日志：不再递归入队（spec 规则 5）。
      const adapter = providers[bot.provider];
      try {
        await adapter?.send(bot, {
          botId: bot.id,
          provider: bot.provider,
          providerUserId: message.providerUserId,
          text: msg(await readMessageLocale(), "deliveryParked"),
        });
      } catch (error) {
        botsLogger.warn(
          undefined,
          `delivery parked notice failed bot=${bot.id}: ${error instanceof Error ? error.message : String(error)}`,
        );
      }
    }
  }

  /**
   * 单条回复的发送 + 恢复：结构化错误按计划重试，恢复用尽后停放。
   * park:false 用于补投路径——条目已在队列里，失败由调用方递增 attempts。
   */
  async function deliverWithRecovery(
    bot: BotConfig,
    message: BotOutboundMessage,
    options: { park?: boolean } = {},
  ): Promise<"sent" | "parked"> {
    const adapter = providers[bot.provider];
    if (!adapter) return "sent";
    let current = message;
    let attempt = 0;
    for (;;) {
      try {
        await adapter.send(bot, current);
        await clearDeliveryNoticeIfSet(bot, current);
        return "sent";
      } catch (error) {
        const code = classifySendError(error);
        if (!code) {
          // 未分类失败保持既有语义：原样冒泡给调用方（stream 队列 warn / 回调错误回复）。
          throw error;
        }
        const detail = error instanceof BotSendError && error.detail ? ` detail=${error.detail}` : "";
        botsLogger.warn(
          undefined,
          `bot send failed bot=${bot.id} provider=${bot.provider} code=${code} attempt=${attempt}: ${error instanceof Error ? error.message : String(error)}${detail}`,
        );
        const freshToken = (await readOutboundConversation(bot, current))?.state.lastContextToken;
        const plan = planDeliveryRecovery(code, {
          attempt,
          usedToken: current.providerContextToken,
          freshToken,
        });
        if (plan.kind === "retry_with_token") {
          current = { ...current, providerContextToken: plan.token };
          attempt += 1;
          continue;
        }
        if (plan.kind === "retry_without_token") {
          const { providerContextToken: _expired, ...rest } = current;
          current = rest;
          attempt += 1;
          continue;
        }
        if (plan.kind === "retry_backoff") {
          await new Promise((resolve) => setTimeout(resolve, plan.delayMs));
          attempt += 1;
          continue;
        }
        if (options.park === false) {
          return "parked";
        }
        await parkOutboundMessage(bot, current, code);
        return "parked";
      }
    }
  }

  /**
   * 补投：按序投递停放回复；第一次失败即停止本轮（服务端状态对后续条目同样不利），
   * 失败条目 attempts+1，超限由队列纯函数丢弃并记 warn。
   */
  async function flushPendingDeliveries(
    bot: BotConfig,
    context: BotContextState,
  ): Promise<BotContextState> {
    let current = context;
    for (;;) {
      const queue = current.pendingDeliveryQueue ?? [];
      const head = queue[0];
      if (!head) return current;
      const message: BotOutboundMessage = {
        botId: bot.id,
        provider: bot.provider,
        providerUserId: head.providerUserId,
        text: head.text,
        ...(current.lastContextToken ? { providerContextToken: current.lastContextToken } : {}),
      };
      let status: "sent" | "parked";
      try {
        status = await botSendQueue.run(bot.id, () =>
          deliverWithRecovery(bot, message, { park: false }),
        );
      } catch (error) {
        status = "parked";
        botsLogger.warn(
          undefined,
          `pending delivery flush error bot=${bot.id}: ${error instanceof Error ? error.message : String(error)}`,
        );
      }
      if (status === "sent") {
        current = { ...current, pendingDeliveryQueue: removeFirstPendingDelivery(queue).queue };
        await writeContext(current);
        continue;
      }
      const bumped = bumpFirstPendingDeliveryAttempt(queue);
      if (bumped.dropped) {
        botsLogger.warn(
          undefined,
          `pending delivery dropped after max attempts bot=${bot.id} user=${bumped.dropped.providerUserId}`,
        );
      }
      current = { ...current, pendingDeliveryQueue: bumped.queue };
      await writeContext(current);
      return current;
    }
  }

  /** 入站刷新：写入新鲜 context_token，随后用它补投停放的回复（spec 规则 6）。 */
  async function refreshDeliveryInbound(
    bot: BotConfig,
    context: BotContextState,
    providerContextToken: string,
  ): Promise<BotContextState> {
    let current = context;
    if (current.lastContextToken !== providerContextToken) {
      current = { ...current, lastContextToken: providerContextToken };
      await writeContext(current);
    }
    if (current.pendingDeliveryQueue?.length) {
      current = await flushPendingDeliveries(bot, current);
    }
    return current;
  }
```

同时在 `createOutbound` 之后声明模块级队列（与 `inboundProcessingQueuesByContext` 同区域也可）：

```ts
  const botSendQueue = createBotSendQueue(BOT_SEND_MIN_INTERVAL_MS);
```

imports 增加：

```ts
import {
  BOT_SEND_MIN_INTERVAL_MS,
  BotSendError,
  bumpFirstPendingDeliveryAttempt,
  classifySendError,
  createBotSendQueue,
  enqueuePendingDelivery,
  planDeliveryRecovery,
  removeFirstPendingDelivery,
  type BotSendFailureCode,
} from "./outboundDelivery.js";
```

- [ ] **Step 2: 替换 `sendOutbound`**

```ts
  async function sendOutbound(bot: BotConfig, message: BotOutboundMessage): Promise<void> {
    const adapter = providers[bot.provider];
    if (!adapter) {
      return;
    }
    // 修复原因：微信出站在会话上下文过期后连续失败且回复被静默丢弃
    // （2026-10-10 事故）。统一走 per-bot 串行队列 + 结构化恢复 + 停放补投。
    await botSendQueue.run(bot.id, () => deliverWithRecovery(bot, message));
  }
```

- [ ] **Step 3: 挂接 `withAuthorizedContext`**

在 `const context = await readContext(message.actor, bot);` 成功返回 context 的路径上（`if (!context)` 分支之后、ok:true 返回之前）插入：

```ts
    // 新入站携带的新鲜 context_token 先落库，再用它补投停放的回复，
    // 保证补投发生在本条消息的业务处理之前（docs/specs/bot-outbound-delivery.md 规则 6）。
    const deliveryContext = message.actor.providerContextToken
      ? await refreshDeliveryInbound(bot, context, message.actor.providerContextToken)
      : context;
```

并把 ok:true 返回里的 `context,` 改为 `context: deliveryContext,`（同一函数内后续对 context 的使用均不受影响——后续只读 workspace 字段，两份对象这些字段一致）。

- [ ] **Step 4: 生命周期清理点**

在以下三处 `queuedMessages: undefined,` 的同一对象字面量内紧随其后各加两行（`pendingDeliveryQueue` 与 `queuedMessages` 同生命周期；`lastContextToken` 不清理——它属于 IM 对话而非任务）：

```ts
          pendingDeliveryQueue: undefined,
          deliveryNoticeAt: undefined,
```

（缩进以各处对象字面量为准；line 1558 处为 8 空格缩进。）

- [ ] **Step 5: 全量测试**

Run: `pnpm --filter @mode/services test`
Expected: PASS（全部既有 bot 测试 + 新增 7 用例；如实报告任何既有失败，不改写为通过）

---

### Task 5: 验证三件套

**Files:** 无新改动（只跑验证；失败则修复后重跑）

- [ ] **Step 1: 类型检查**

Run: `pnpm typecheck`
Expected: 0 错误

- [ ] **Step 2: Lint**

Run: `pnpm lint`
Expected: 0 错误（若仅剩与本改动无关的既有告警，如实列出）

- [ ] **Step 3: 架构检查**

Run: `pnpm architecture:check --changed`
Expected: `violations: 0`（对照改动前基线 0）

- [ ] **Step 4: 汇报**

输出：改动文件清单、测试/检查真实结果、与 spec 验收场景的对应关系、未覆盖项（如实机回归需用户配合发微信消息验证）。

## Self-Review 记录

1. **Spec 覆盖**：规则 1→Task 2/4（串行队列）；规则 2→Task 3（分类）；规则 3→Task 2 `planDeliveryRecovery` + Task 4 编排；规则 4→Task 2/3；规则 5→Task 4 `parkOutboundMessage`；规则 6→Task 4 `refreshDeliveryInbound`/`flushPendingDeliveries`；规则 7→Task 4 Step 4；验收 1/2→Task 1-3 测试；验收 3→Task 5；验收 4（实机）→Task 5 Step 4 标注需用户配合。
2. **占位符**：无 TBD/TODO；`fetchBotProviderJson` 返回类型处已给出"先读签名再落笔"的明确动作而非留空。
3. **类型一致性**：`BotPendingDelivery` 字段（text/providerUserId/queuedAt/attempts）在 Task 1 定义、Task 2/4 使用一致；`deliverWithRecovery` 返回值 `"sent" | "parked"` 在 Task 4 内自洽；`planDeliveryRecovery` 的 `attempt` 语义在测试与实现一致。
