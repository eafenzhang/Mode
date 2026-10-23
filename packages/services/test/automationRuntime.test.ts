import assert from "node:assert/strict";
import test, { after } from "node:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ModelSelection } from "@mode/shared";

// Automation 运行时（docs/specs/automation-runtime.md）的结算状态机回归：
// 1) 到期 schedule 认领 → 派发 → markDispatched 推进 next_run_at 并累计 run_count；
// 2) 派发失败 → markDispatchFailed transient 退避、复位 running；
// 3) misfire（首轮且过期超宽限）→ skipAndReschedule 记 skipped，一次性任务错过即终态；
// 4) manual 直派成功 → markManualRunDispatched，claim 保留到真实 turn 终态才释放；
// 5) manual 直派失败 → 释放 claim 并向调用方 rethrow（UI 显示「触发运行失败」）。
// 数据隔离：AutomationRepo 直接注入 temp 库路径，不依赖 MODE_DATA_BASE_DIR；
// 但 @mode/services 根入口的模块加载会读 paths，仍需先设 env 再动态 import。
const dataDir = mkdtempSync(join(tmpdir(), "mode-automation-runtime-"));
process.env.MODE_DATA_BASE_DIR = dataDir;
after(() => {
  try {
    rmSync(dataDir, { recursive: true, force: true, maxRetries: 3, retryDelay: 100 });
  } catch {
    // Windows 下 sqlite 句柄可能延迟释放；清理失败不影响断言语义。
  }
});

const node = await import("@mode/services/node");
const services = await import("@mode/services");

const { createAutomationRuntime, AutomationRepo } = node;
const { ServiceCollection, IModeTaskService, IModelSelectionService, IBotsService } = services;

const WORKSPACE = join(dataDir, "ws");
const MODEL_SELECTION: ModelSelection = {
  providerId: "test-provider",
  modelId: "test-model",
  options: { reasoningLevel: "medium" },
};

/** IModeTaskService 的最小桩：捕获 sendPrompt 与终态订阅，供测试驱动。 */
function createTaskServiceStub() {
  const terminalListeners = new Map<string, Array<(event: unknown) => void>>();
  const prompts: Array<{ taskId: string; traceId: string; content: string }> = [];
  let sendPromptError: Error | null = null;
  return {
    prompts,
    failSendPromptWith(error: Error | null) {
      sendPromptError = error;
    },
    fireTerminal(taskId: string, event: { inputId?: string; outcome: string; error?: string }) {
      for (const listener of terminalListeners.get(taskId) ?? []) {
        listener(event);
      }
    },
    stub: {
      async createTask() {
        return { taskId: `task-${prompts.length}-${Date.now()}` };
      },
      async resumeTask(params: { taskId: string }) {
        return { taskId: params.taskId };
      },
      async setAutomationSessionConfig() {
        return [];
      },
      async sendPrompt(params: { taskId: string; traceId: string; content: string }) {
        if (sendPromptError) throw sendPromptError;
        prompts.push({
          taskId: params.taskId,
          traceId: params.traceId,
          content: params.content,
        });
      },
      onDynamicTaskTerminalOutcome(taskId: string) {
        return (listener: (event: unknown) => void) => {
          const list = terminalListeners.get(taskId) ?? [];
          list.push(listener);
          terminalListeners.set(taskId, list);
          return {
            dispose() {
              const remaining = (terminalListeners.get(taskId) ?? []).filter(
                (entry) => entry !== listener,
              );
              terminalListeners.set(taskId, remaining);
            },
          };
        };
      },
      async setTaskUnread() {
        return { taskId: "stub" } as never;
      },
    } as unknown as typeof IModeTaskService,
  };
}

function createModelSelectionStub(options: {
  effectiveSelection?: typeof MODEL_SELECTION | null;
  preferredSelection?: typeof MODEL_SELECTION | null;
  selectionIssue?: string;
}) {
  return {
    async getView(input?: { selection?: typeof MODEL_SELECTION }) {
      if (input?.selection) {
        return {
          effectiveSelection: options.effectiveSelection ?? input.selection,
          preferredSelection: options.preferredSelection ?? null,
          ...(options.selectionIssue ? { selectionIssue: options.selectionIssue } : {}),
        };
      }
      return {
        effectiveSelection: null,
        preferredSelection: options.preferredSelection ?? null,
      };
    },
  } as unknown as typeof IModelSelectionService;
}

interface TestHarness {
  repo: InstanceType<typeof AutomationRepo>;
  runtime: ReturnType<typeof createAutomationRuntime>;
  taskStub: ReturnType<typeof createTaskServiceStub>;
  dispose(): Promise<void>;
}

async function createHarness(
  modelSelectionStub?: ReturnType<typeof createModelSelectionStub>,
): Promise<TestHarness> {
  const dbPath = join(
    dataDir,
    `runtime-${Date.now()}-${Math.random().toString(36).slice(2)}.sqlite`,
  );
  const repo = new AutomationRepo(dbPath);
  await repo.ensureReady();
  const taskStub = createTaskServiceStub();
  const collection = new ServiceCollection();
  collection.register(IModeTaskService, taskStub.stub);
  collection.register(
    IModelSelectionService,
    modelSelectionStub ?? createModelSelectionStub({ effectiveSelection: MODEL_SELECTION }),
  );
  collection.register(IBotsService, {
    async watchAutomationRun() {
      // Bot 回推是 best-effort：桩不参与断言。
    },
  } as never);
  const runtime = createAutomationRuntime({
    repo,
    resolveTargetServices: () => collection,
  });
  return {
    repo,
    runtime,
    taskStub,
    async dispose() {
      await runtime.stop();
      repo.close();
    },
  };
}

async function createDueAutomation(
  repo: InstanceType<typeof AutomationRepo>,
  options: { recurring?: boolean; nextRunAt?: number } = {},
) {
  return repo.create(
    {
      title: "test automation",
      cronExpr: "*/10 * * * *",
      prompt: "推进路线图",
      workspacePath: WORKSPACE,
      recurring: options.recurring ?? true,
      modelSelection: MODEL_SELECTION,
    },
    { nextRunAt: options.nextRunAt ?? Date.now() - 1_000 },
  );
}

/** 等待异步结算回写（终态 handler 是 fire-and-forget，轮询 DB 直到条件成立）。 */
async function waitFor(
  predicate: () => Promise<boolean> | boolean,
  timeoutMs = 2_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    if (await predicate()) return;
    if (Date.now() > deadline) throw new Error("waitFor 超时");
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
}

test("schedule 到期认领 → 派发 → markDispatched 推进 next_run_at 并累计 run_count", async () => {
  const harness = await createHarness();
  try {
    const automation = await createDueAutomation(harness.repo);
    await harness.runtime.wake();

    assert.equal(harness.taskStub.prompts.length, 1, "应产生一次 sendPrompt");
    const runId = `${automation.automationId}:${automation.nextRunAt}`;
    assert.equal(harness.taskStub.prompts[0]!.traceId, runId, "traceId 必须是 runId");

    const settled = await harness.repo.get(automation.automationId);
    assert.ok(settled);
    // running 是 DB 内部列，不在 ModeAutomation 接口上；markDispatched 的契约已保证复位
    // running，这里用 dispatchStatus + runNow 可再认领作为可观测代理。
    assert.equal(settled.dispatchStatus, "dispatched");
    assert.equal(settled.runCount, 1, "run_count 累计");
    assert.ok(
      (settled.nextRunAt ?? 0) > Date.now() - 60_000,
      "next_run_at 必须推进到未来（cron */10 下一整十分）",
    );
    const reClaim = await harness.repo.runNow(automation.automationId, { now: Date.now() });
    assert.ok(reClaim, "派发结算后 single-flight 已释放，可再次认领");
    await harness.repo.releaseManualClaim(automation.automationId, settled.workspaceKey);

    const run = await harness.repo.getRun(runId);
    assert.ok(run);
    assert.equal(run.dispatchStatus, "dispatched");
    assert.equal(run.trigger, "schedule");
  } finally {
    await harness.dispose();
  }
});

test("schedule 派发失败 → markDispatchFailed transient 退避并复位 running", async () => {
  const harness = await createHarness();
  try {
    const automation = await createDueAutomation(harness.repo);
    harness.taskStub.failSendPromptWith(new Error("agent not ready"));
    await harness.runtime.wake();

    const settled = await harness.repo.get(automation.automationId);
    assert.ok(settled);
    assert.equal(settled.dispatchStatus, "failed_to_dispatch");
    assert.equal(settled.dispatchAttempts, 1, "transient 失败累计尝试次数");
    assert.ok(settled.retryAt !== null && settled.retryAt > Date.now(), "应写入退避 retry_at");
    const reClaim = await harness.repo.runNow(automation.automationId, { now: Date.now() });
    assert.ok(reClaim, "失败结算后 single-flight 已释放，可再次认领");
    await harness.repo.releaseManualClaim(automation.automationId, settled.workspaceKey);

    const runId = `${automation.automationId}:${automation.nextRunAt}`;
    const run = await harness.repo.getRun(runId);
    assert.ok(run);
    assert.equal(run.dispatchStatus, "failed_to_dispatch");
    assert.match(run.error ?? "", /agent not ready/);
  } finally {
    await harness.dispose();
  }
});

test("misfire：首轮且过期超宽限 → skipAndReschedule 记 skipped；一次性任务错过即终态", async () => {
  const harness = await createHarness();
  try {
    // 过期 10 分钟 > 默认 5 分钟 misfire 宽限，且 dispatchAttempts=0（首轮）。
    const longMissed = await createDueAutomation(harness.repo, {
      nextRunAt: Date.now() - 10 * 60_000,
    });
    // 一次性任务：recurring=false 且未显式 max_runs 时按上限 1 处理。
    const oneShot = await createDueAutomation(harness.repo, {
      recurring: false,
      nextRunAt: Date.now() - 10 * 60_000,
    });
    await harness.runtime.wake();

    assert.equal(harness.taskStub.prompts.length, 0, "错过窗口不补跑");

    const skippedRun = await harness.repo.getRun(
      `${longMissed.automationId}:${longMissed.nextRunAt}`,
    );
    assert.ok(skippedRun);
    assert.equal(skippedRun.dispatchStatus, "skipped");
    const rescheduled = await harness.repo.get(longMissed.automationId);
    assert.ok(rescheduled);
    assert.equal(rescheduled.dispatchStatus, "idle", "skip 后调度状态复位 idle");
    assert.equal(rescheduled.lifecycleStatus, "active");
    assert.ok((rescheduled.nextRunAt ?? 0) > Date.now() - 60_000, "循环任务跳到下一个未来触发点");

    const finalized = await harness.repo.get(oneShot.automationId);
    assert.ok(finalized);
    assert.equal(finalized.lifecycleStatus, "completed", "一次性任务错过即终态");
    assert.equal(finalized.enabled, false, "一次性任务终态后停用");
    assert.equal(finalized.nextRunAt ?? null, null, "终态后不再排下一次触发");
  } finally {
    await harness.dispose();
  }
});

test("manual 直派成功 → markManualRunDispatched；turn 终态释放 claim 并回写 outcome", async () => {
  const harness = await createHarness();
  try {
    const { automation, run } = (await harness.repo.runNow(
      (
        await createDueAutomation(harness.repo)
      ).automationId,
      { now: Date.now() },
    ))!;
    await harness.runtime.dispatchManualRun({ automation, run });

    assert.equal(harness.taskStub.prompts.length, 1);
    assert.equal(harness.taskStub.prompts[0]!.traceId, run.runId, "manual traceId 必须是 runId");

    const dispatched = await harness.repo.getRun(run.runId);
    assert.ok(dispatched);
    assert.equal(dispatched.dispatchStatus, "dispatched");
    const claimed = await harness.repo.get(automation.automationId);
    assert.ok(claimed);
    assert.equal(claimed.runCount, 1, "manual 派发累计 run_count");
    // claim 保留到真实 turn 终态：single-flight 仍被占用，runNow 必须被拒。
    const blocked = await harness.repo.runNow(automation.automationId, { now: Date.now() });
    assert.equal(blocked, null, "turn 终态前 single-flight 仍被占用");

    // 真实 turn 终态：回写 outcome + 释放 single-flight claim（异步回写，轮询等待）。
    harness.taskStub.fireTerminal(harness.taskStub.prompts[0]!.taskId, {
      inputId: run.runId,
      outcome: "succeeded",
    });
    await waitFor(async () => {
      const current = await harness.repo.getRun(run.runId);
      return current?.outcome === "succeeded";
    });
    const settledRun = await harness.repo.getRun(run.runId);
    assert.ok(settledRun);
    assert.equal(settledRun.outcome, "succeeded", "终态回写 run outcome");
    // 终态 handler 在 markRunOutcome 之后串行释放 claim；轮询到「能再次认领」为止。
    await waitFor(async () => {
      const reClaim = await harness.repo.runNow(automation.automationId, { now: Date.now() });
      if (!reClaim) return false;
      await harness.repo.releaseManualClaim(
        automation.automationId,
        reClaim.automation.workspaceKey,
      );
      return true;
    });
  } finally {
    await harness.dispose();
  }
});

test("manual 直派失败 → 释放 claim 并向调用方 rethrow", async () => {
  const harness = await createHarness(
    createModelSelectionStub({ selectionIssue: "provider missing" }),
  );
  try {
    const { automation, run } = (await harness.repo.runNow(
      (
        await createDueAutomation(harness.repo)
      ).automationId,
      { now: Date.now() },
    ))!;

    await assert.rejects(
      () => harness.runtime.dispatchManualRun({ automation, run }),
      /Automation 模型选择不可用/,
      "派发错误必须透传给调用方（UI 显示触发失败）",
    );
    // dispatchManualRun 的 finally 会唤醒轮询；等待在途 tick 结算后再断言认领状态，
    // 否则 tick 可能正持有该 automation 的 schedule 认领。
    await harness.runtime.wake();

    const failedRun = await harness.repo.getRun(run.runId);
    assert.ok(failedRun);
    assert.equal(failedRun.dispatchStatus, "failed_to_dispatch");
    // 失败必须释放 single-flight claim：runNow 能再次认领即为可观测证据。
    const reClaim = await harness.repo.runNow(automation.automationId, { now: Date.now() });
    assert.ok(reClaim, "失败必须释放 single-flight claim，避免卡死后续触发");
    await harness.repo.releaseManualClaim(
      automation.automationId,
      reClaim!.automation.workspaceKey,
    );
  } finally {
    await harness.dispose();
  }
});

test("同 automation 已在途时 runNow 返回 null（single-flight）", async () => {
  const harness = await createHarness();
  try {
    const automation = await createDueAutomation(harness.repo);
    // 手动占用 running 锁，模拟已有 turn 在途。
    const claimed = await harness.repo.runNow(automation.automationId, { now: Date.now() });
    assert.ok(claimed, "首次 runNow 应认领成功");
    const second = await harness.repo.runNow(automation.automationId, { now: Date.now() });
    assert.equal(second, null, "single-flight 拒绝并发 manual run");
  } finally {
    await harness.dispose();
  }
});
