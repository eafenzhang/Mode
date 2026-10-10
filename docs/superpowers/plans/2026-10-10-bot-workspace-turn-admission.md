# Bot 同 workspace 回合并入上限 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 同一 workspace 内 bot 发起的运行中回合 ≤ 2，超出按 FIFO 静默排队。

**Architecture:** 纯内存门 `createWorkspaceTurnGate`（taskId→workspaceKey 跟踪 + 每 workspace FIFO 等待队列），接入点与 `runningTasks` 严格成对：`add` 前 `enter`、每个 `delete/clear` 旁 `exit/reset`，同生共死不产生第二套真相。

**Tech Stack:** TypeScript ESM、node:test（`tsx --test`）。

**Spec:** `docs/specs/bot-workspace-turn-admission.md`（执行者需同时阅读）

## Global Constraints

- 上限常量 `BOT_WORKSPACE_TURN_CONCURRENCY_CAP = 2`；等待无超时（spec 规则 5，禁止超时兜底）。
- 不改本对话既有排队语义；不加用户提示（v1 已知限制）。
- 不执行 `git commit` / `git push`。
- 验证：`pnpm --filter @mode/services test`、`pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`。

---

### Task 1: `workspaceTurnGate` 纯模块 + 单测

**Files:**
- Create: `packages/services/src/bots/workspaceTurnGate.ts`
- Test: `packages/services/test/bot-workspace-turn-gate.test.ts`（新建）

**Interfaces:**
- Produces（Task 2 依赖）:
  - `export const BOT_WORKSPACE_TURN_CONCURRENCY_CAP = 2`
  - `export interface WorkspaceTurnGate { enter(workspaceKey: string, taskId: string): Promise<void>; exit(taskId: string): void; reset(): void; count(workspaceKey: string): number }`
  - `export function createWorkspaceTurnGate(cap?: number): WorkspaceTurnGate`
  - 语义：`enter` 幂等（同 taskId 重复进入立即返回）；count≥cap 时入该 workspace 的 FIFO 等待队列，被 `exit/reset` 唤醒后**重新检查**计数；`exit` 未知 taskId 为 no-op；`reset` 清空跟踪并唤醒全部等待者

- [ ] **Step 1: 写失败测试** `packages/services/test/bot-workspace-turn-gate.test.ts`：

```ts
import assert from "node:assert/strict";
import test from "node:test";

// 背景：多用户并发首发同一 workspace 的 bot 回合无准入控制，冷启动尖峰
// 由平台限流与资源竞争兜底（docs/specs/bot-workspace-turn-admission.md）。

const flush = () => new Promise((resolve) => setImmediate(resolve));

test("cap=2：第三个 enter 阻塞，exit 后按 FIFO 顺序放行", async () => {
  const { createWorkspaceTurnGate } = await import("../src/bots/workspaceTurnGate.js");
  const gate = createWorkspaceTurnGate(2);
  await gate.enter("/w1", "t1");
  await gate.enter("/w1", "t2");
  assert.equal(gate.count("/w1"), 2);

  let thirdDone = false;
  let fourthDone = false;
  const third = gate.enter("/w1", "t3").then(() => {
    thirdDone = true;
  });
  const fourth = gate.enter("/w1", "t4").then(() => {
    fourthDone = true;
  });
  await flush();
  assert.equal(thirdDone, false, "cap 满时第三个必须等待");
  assert.equal(fourthDone, false);

  gate.exit("t1");
  await third;
  assert.equal(thirdDone, true);
  await flush();
  assert.equal(fourthDone, false, "FIFO：t3 先占坑，t4 仍等待");
  assert.equal(gate.count("/w1"), 2);

  gate.exit("t2");
  await fourth;
  assert.equal(fourthDone, true);
  assert.equal(gate.count("/w1"), 2);
});

test("不同 workspace 互不影响；enter 幂等；exit 未知/no-op", async () => {
  const { createWorkspaceTurnGate } = await import("../src/bots/workspaceTurnGate.js");
  const gate = createWorkspaceTurnGate(1);
  await gate.enter("/w1", "t1");
  // w2 独立计数，立即放行
  await gate.enter("/w2", "t2");
  assert.equal(gate.count("/w1"), 1);
  assert.equal(gate.count("/w2"), 1);

  // 同 taskId 重复 enter 幂等（不占双份槽位、不阻塞）
  await gate.enter("/w1", "t1");
  assert.equal(gate.count("/w1"), 1);

  // 未知与重复 exit 均为 no-op，不抛错
  gate.exit("unknown");
  gate.exit("t1");
  gate.exit("t1");
  assert.equal(gate.count("/w1"), 0);
});

test("reset 清空跟踪并唤醒全部等待者，唤醒后按当前计数重新竞争", async () => {
  const { createWorkspaceTurnGate } = await import("../src/bots/workspaceTurnGate.js");
  const gate = createWorkspaceTurnGate(1);
  await gate.enter("/w1", "t1");
  let wokenA = false;
  let wokenB = false;
  const a = gate.enter("/w1", "wa").then(() => {
    wokenA = true;
  });
  const b = gate.enter("/w1", "wb").then(() => {
    wokenB = true;
  });
  await flush();
  assert.equal(wokenA, false);

  gate.reset();
  assert.equal(gate.count("/w1"), 0);
  await a;
  await flush();
  // cap=1：a 占坑后 b 仍需等待（唤醒≠无条件放行）
  assert.equal(wokenA, true);
  assert.equal(wokenB, false);
  gate.exit("wa");
  await b;
  assert.equal(wokenB, true);
});
```

- [ ] **Step 2: 运行确认失败**

Run: `pnpm --filter @mode/services exec tsx --test test/bot-workspace-turn-gate.test.ts`
Expected: FAIL（模块不存在）

- [ ] **Step 3: 实现** `packages/services/src/bots/workspaceTurnGate.ts`：

```ts
/**
 * 同 workspace 的 bot 回合并入门（纯内存，无 IO）。
 * 多用户并发首发同一 workspace 时 N 个 agent 回合同时起跑造成资源/限流尖峰
 * （docs/specs/bot-workspace-turn-admission.md）。
 */

/** 同一 workspace 内由 bot 侧发起的运行中回合并入上限。 */
export const BOT_WORKSPACE_TURN_CONCURRENCY_CAP = 2;

export interface WorkspaceTurnGate {
  /** 获取槽位；超限时入该 workspace 的 FIFO 等待队列。同 taskId 幂等。 */
  enter(workspaceKey: string, taskId: string): Promise<void>;
  /** 释放槽位；未知/重复 taskId 为 no-op，并唤醒该 workspace 队首等待者。 */
  exit(taskId: string): void;
  /** 清空全部跟踪并唤醒所有等待者（进程收尾/对账用）。 */
  reset(): void;
  count(workspaceKey: string): number;
}

interface Waiter {
  workspaceKey: string;
  resolve: () => void;
}

export function createWorkspaceTurnGate(
  cap: number = BOT_WORKSPACE_TURN_CONCURRENCY_CAP,
): WorkspaceTurnGate {
  const limit = Math.max(1, cap);
  const workspaceByTaskId = new Map<string, string>();
  const waiters: Waiter[] = [];

  const count = (workspaceKey: string): number => {
    let total = 0;
    for (const key of workspaceByTaskId.values()) {
      if (key === workspaceKey) {
        total += 1;
      }
    }
    return total;
  };

  const wakeNext = (workspaceKey: string): void => {
    const index = waiters.findIndex((waiter) => waiter.workspaceKey === workspaceKey);
    if (index < 0) {
      return;
    }
    const [waiter] = waiters.splice(index, 1);
    waiter?.resolve();
  };

  return {
    async enter(workspaceKey: string, taskId: string): Promise<void> {
      if (workspaceByTaskId.has(taskId)) {
        return;
      }
      // 检查与登记之间没有 await，检查→入队→挂起是原子的：
      // 期间不可能有 exit 被处理后漏唤醒（exit 只能发生在我们挂起之后）。
      while (count(workspaceKey) >= limit) {
        await new Promise<void>((resolve) => {
          waiters.push({ workspaceKey, resolve });
        });
        // 被唤醒后重新检查计数：可能已被同 workspace 的其他等待者抢走槽位。
        if (workspaceByTaskId.has(taskId)) {
          return;
        }
      }
      workspaceByTaskId.set(taskId, workspaceKey);
    },
    exit(taskId: string): void {
      const workspaceKey = workspaceByTaskId.get(taskId);
      workspaceByTaskId.delete(taskId);
      if (workspaceKey !== undefined) {
        wakeNext(workspaceKey);
      }
    },
    reset(): void {
      workspaceByTaskId.clear();
      const pending = waiters.splice(0, waiters.length);
      for (const waiter of pending) {
        waiter.resolve();
      }
    },
    count,
  };
}
```

- [ ] **Step 4: 运行确认通过**

Run: `pnpm --filter @mode/services exec tsx --test test/bot-workspace-turn-gate.test.ts`
Expected: PASS（3 用例）

### Task 2: botsService 接入（enter/exit 与 runningTasks 成对）

**Files:**
- Modify: `packages/services/src/bots/botsService.ts`（`runningTasks` 声明 ~821；add 点 2 处、delete 点 4 处、clear 1 处——以函数语义定位）

- [ ] **Step 1: 声明与导入**：`import { createWorkspaceTurnGate } from "./workspaceTurnGate.js";`；
  在 `const runningTasks = new Set<string>();`（~line 821）旁新增
  `const turnGate = createWorkspaceTurnGate();`。

- [ ] **Step 2: 两个 enter 点**（各在 `runningTasks.add` 正前一行）：

冷启动分支（`runningTasks.add(task.taskId)` 前）：

```ts
      // 同 workspace 回合并入上限：超限静默排队，终态释放后按序起跑
      // （docs/specs/bot-workspace-turn-admission.md）。
      await turnGate.enter(
        getWorkspaceKey(context.workspacePath, context.workspaceIdentity),
        task.taskId,
      );
      runningTasks.add(task.taskId);
```

续跑分支（`runningTasks.add(auth.context.activeTaskId)` 前）：

```ts
    await turnGate.enter(
      getWorkspaceKey(auth.context.workspacePath, auth.context.workspaceIdentity),
      auth.context.activeTaskId,
    );
    runningTasks.add(auth.context.activeTaskId);
```

- [ ] **Step 3: 四个 exit 点 + reset**（每处紧邻 `runningTasks.delete|clear`，保持成对）：

1. 终态事件（`task_complete || task_error` 分支，`runningTasks.delete(event.taskId);` 后）
   → `turnGate.exit(event.taskId);`
2. 发送失败 catch（`runningTasks.delete(taskId);` 后）→ `turnGate.exit(taskId);`
3. staleness 对账（`runningTasks.delete(context.activeTaskId);` 后）
   → `turnGate.exit(context.activeTaskId);`
4. /停止 成功（`runningTasks.delete(auth.context.activeTaskId);` 后）
   → `turnGate.exit(auth.context.activeTaskId);`
5. dispose（`runningTasks.clear();` 后）→ `turnGate.reset();`

- [ ] **Step 4: 成对完整性检查**

Run: `rg -n "runningTasks\.(delete|clear)" packages/services/src/bots/botsService.ts`
Expected: 每行邻近（同分支数行内）有对应 `turnGate.exit|reset` 调用——逐行人工核对。

- [ ] **Step 5: 全量测试**

Run: `pnpm --filter @mode/services test`
Expected: PASS（新增 3 用例 + 全部既有；如实报告失败）

### Task 3: 验证三件套 + 汇报

- [ ] `pnpm typecheck` → 0 错误
- [ ] `pnpm lint` → 0 错误
- [ ] `pnpm architecture:check --changed` → violations 0
- [ ] 汇报：改动清单、真实结果、验收对应、残余风险（跨对话等待无提示；泄漏语义与 runningTasks 同生共死）

## Self-Review 记录

1. **Spec 覆盖**：规则 1→常量+Task1 cap；规则 2→enter 排队语义+无超时；规则 3→Step2/3 成对；
   规则 4→workspace 隔离测试；规则 5→无超时实现；规则 6→无提示（非目标）；验收 1→Task1 测试；
   验收 2→Step4 人工核对；验收 3→Task3。
2. **占位符**：无。
3. **类型一致性**：`WorkspaceTurnGate` 三个方法在 Task1 定义、Task2 消费一致；
   `getWorkspaceKey` 在两个 enter 点均可用（同函数既有引用）。
