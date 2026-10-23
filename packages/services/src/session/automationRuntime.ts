/* eslint-disable max-lines -- automation 运行时集中承载「认领→派发→结算」的进程内调度状态机，
   与 AutomationRepo 的状态机契约一一对应；稳定后再按派发/轮询职责拆分。 */
/**
 * Automation 运行时（Tauri 单进程派发链）。
 *
 * spec: docs/specs/automation-runtime.md。Electron 时代这条链由「独立 scheduler 进程 +
 * main 消息路由 + host 派发」三件组成；Tauri host-standalone 是单 Node sidecar 进程，
 * 调度循环、派发与结算合并为进程内模块，状态机仍以 AutomationRepo 为唯一写入方。
 */
import {
  resolveWorkspaceKey,
  type ModeAutomation,
  type ModeAutomationRun,
  type ModeAutomationTrigger,
  type ModelSelection,
  type ModeTaskMode,
  type TraceId,
} from "@mode/shared";
import { computeAutomationNextRunAt, isOneShotAutomation } from "#src/session/automationCron.js";
import { AutomationRepo } from "#src/session/automationRepo.js";
import { IModeTaskService, type ModeTaskTerminalOutcome } from "#src/session/modeTaskService.js";
import { IModelSelectionService } from "#src/model-provider/providerFacadeServices.js";
import { IBotsService } from "#src/bots/bots.js";
import { formatModelPickerValue } from "#src/mode-agent/modeConfigOptions.js";
import type { ServiceCollection } from "#src/collection.js";

/** 轮询间隔：cron 最小粒度是分钟，20s 轮询足以按时命中且开销低。 */
const POLL_INTERVAL_MS = 20_000;
/**
 * misfire 宽限：next_run_at 早于 now 超过该值视为「关机/休眠期间错过的窗口」→ 记 skipped
 * 不补跑。取值需明显大于一次正常轮询延迟，又能覆盖短暂卡顿。
 */
const MISFIRE_GRACE_MS = 5 * 60_000;
/** manual claim 心跳间隔：长任务 turn 执行期间续租，防被僵尸回收。 */
const MANUAL_CLAIM_HEARTBEAT_MS = 60_000;

export interface AutomationRuntimeLog {
  (level: "info" | "warn" | "error", message: string): void;
}

export interface CreateAutomationRuntimeOptions {
  /** 缺省新建；测试可注入临时库路径的 repo。 */
  repo?: AutomationRepo;
  /**
   * 按 workspace 解析目标服务集合：本地 workspace 返回本地 services；远端已连接返回该会话
   * 的 scoped services；远端 identity 无可用会话必须抛错，禁止静默回退本地。
   */
  resolveTargetServices: (request: {
    workspacePath: string;
    workspaceIdentity?: string;
  }) => ServiceCollection;
  log?: AutomationRuntimeLog;
  pollIntervalMs?: number;
  misfireGraceMs?: number;
  claimHeartbeatMs?: number;
}

export interface AutomationRuntime {
  /** 启动轮询并立即执行一轮认领；幂等。 */
  start(): void;
  /** 停止轮询、退订终态、释放本进程在途认领；幂等。 */
  stop(): Promise<void>;
  /** 请求立即触发一轮认领并等待其完成（manual run 落库后调用，避免等下一个轮询周期）。 */
  wake(): Promise<void>;
  /**
   * 「立即运行」直派入口：modeAgentService 认领（repo.runNow）后回调。派发成功结算
   * manual run 台账（不释放 single-flight claim，等真实 turn 终态）；失败释放 claim 并 rethrow，
   * 让 UI 显示「触发运行失败」。
   */
  dispatchManualRun(params: { automation: ModeAutomation; run: ModeAutomationRun }): Promise<void>;
}

/** 一次派发请求的完整上下文（runId 即 traceId，也是 run 台账主键）。 */
interface AutomationDispatchRequest {
  automationId: string;
  runId: string;
  prompt: string;
  targetTaskId?: string;
  modelSelection?: ModelSelection;
  mode?: ModeTaskMode;
  workspacePath: string;
  workspaceIdentity?: string;
  workspaceKey: string;
  trigger: ModeAutomationTrigger;
  scheduledAt: number | null;
}

interface AutomationDispatchResult {
  taskId: string;
  sessionId: string;
}

/** 在途终态订阅：manual 附加 claim 心跳；两者在终态或 stop 时一并释放。 */
interface ActiveRunTracking {
  trigger: ModeAutomationTrigger;
  automationId: string;
  workspaceKey: string;
  disposeHeartbeat: () => void;
  disposeSubscription: () => void;
}

function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function createAutomationRuntime(
  options: CreateAutomationRuntimeOptions,
): AutomationRuntime {
  const repo = options.repo ?? new AutomationRepo();
  const log: AutomationRuntimeLog = options.log ?? (() => undefined);
  const pollIntervalMs = options.pollIntervalMs ?? POLL_INTERVAL_MS;
  const misfireGraceMs = options.misfireGraceMs ?? MISFIRE_GRACE_MS;
  const claimHeartbeatMs = options.claimHeartbeatMs ?? MANUAL_CLAIM_HEARTBEAT_MS;

  let pollTimer: ReturnType<typeof setInterval> | null = null;
  let ticking = false;
  let tickRequested = false;
  let disposed = false;
  let started = false;
  /** runId → 在途终态订阅；stop 时统一退订并释放 claim。 */
  const activeTrackings = new Map<string, ActiveRunTracking>();
  /** 本 tick 已认领但尚未结算的 schedule automationId；stop 时释放认领。 */
  const inFlightScheduleClaims = new Set<string>();

  function buildScheduledRunId(automationId: string, scheduledAt: number): string {
    return `${automationId}:${scheduledAt}`;
  }

  /** 订阅真实 turn 终态：回写 run outcome、已读标记；manual 释放 single-flight claim。 */
  function trackRunOutcome(
    targetServices: ServiceCollection,
    request: AutomationDispatchRequest,
    taskId: string,
  ): void {
    const modeTaskService = targetServices.getOptional(IModeTaskService);
    if (!modeTaskService) {
      throw new Error("Mode task service is not initialized.");
    }
    const traceId = request.runId as TraceId;
    // manual claim 覆盖 queue 等待与 turn 执行，只能在真实终态后释放；schedule claim 由
    // markDispatched/markDispatchFailed 在派发结算时复位，终态只回写 outcome。
    void (async () => {
      try {
        await repo.ensureRunClaimed({
          runId: request.runId,
          automationId: request.automationId,
          workspaceKey: request.workspaceKey,
          scheduledAt: request.scheduledAt,
          trigger: request.trigger,
        });
        await repo.markRunOutcome(request.runId, "running");
      } catch (error) {
        log(
          "warn",
          `回写定时任务 running 状态失败 runId=${request.runId}: ${describeError(error)}`,
        );
      }
    })();

    const heartbeatTimer =
      request.trigger === "manual"
        ? setInterval(() => {
            void repo
              .touchManualClaim(request.automationId, request.workspaceKey)
              .catch((error) => {
                log(
                  "warn",
                  `续租 manual automation claim 失败 automation=${request.automationId}: ${describeError(error)}`,
                );
              });
          }, claimHeartbeatMs)
        : null;

    const disposeSubscription = modeTaskService.onDynamicTaskTerminalOutcome(taskId)((
      result: ModeTaskTerminalOutcome,
    ) => {
      if (result.inputId !== traceId) return;
      void (async () => {
        // outcome 回写与 claim 释放互相独立：任一步失败都不能把 single-flight 锁卡到
        // CLAIM_STALE（10 分钟）才回收，所以分开 try，释放单独兜底。
        try {
          await repo.ensureRunClaimed({
            runId: request.runId,
            automationId: request.automationId,
            workspaceKey: request.workspaceKey,
            scheduledAt: request.scheduledAt,
            trigger: request.trigger,
          });
          await repo.markRunOutcome(request.runId, result.outcome, result.error);
        } catch (error) {
          log("warn", `回写定时任务终态失败 runId=${request.runId}: ${describeError(error)}`);
        }
        if (request.trigger === "manual") {
          try {
            await repo.releaseManualClaim(request.automationId, request.workspaceKey);
          } catch (error) {
            log(
              "warn",
              `释放 manual automation claim 失败 automation=${request.automationId}: ${describeError(error)}`,
            );
          }
        }
        try {
          await modeTaskService.setTaskUnread({
            taskId,
            workspacePath: request.workspacePath,
            ...(request.workspaceIdentity ? { workspaceIdentity: request.workspaceIdentity } : {}),
            unread: true,
          });
        } catch (error) {
          log("warn", `标记任务已读失败 taskId=${taskId}: ${describeError(error)}`);
        }
        const tracking = activeTrackings.get(request.runId);
        tracking?.disposeHeartbeat();
        activeTrackings.delete(request.runId);
        disposeSubscription.dispose();
      })();
    });

    activeTrackings.set(request.runId, {
      trigger: request.trigger,
      automationId: request.automationId,
      workspaceKey: request.workspaceKey,
      disposeHeartbeat: () => {
        if (heartbeatTimer) clearInterval(heartbeatTimer);
      },
      disposeSubscription: () => disposeSubscription.dispose(),
    });
  }

  /** 核心派发：解析模型 → 建/恢复 task → Bot 回推订阅 → 终态订阅 → sendPrompt。 */
  async function dispatchRun(
    request: AutomationDispatchRequest,
  ): Promise<AutomationDispatchResult> {
    const targetServices = options.resolveTargetServices({
      workspacePath: request.workspacePath,
      ...(request.workspaceIdentity ? { workspaceIdentity: request.workspaceIdentity } : {}),
    });
    const modeTaskService = targetServices.getOptional(IModeTaskService);
    if (!modeTaskService) {
      throw new Error("Mode task service is not initialized.");
    }
    const modelSelectionService = targetServices.getOptional(IModelSelectionService);
    if (!modelSelectionService) {
      throw new Error("Model Selection service is not initialized.");
    }

    // 模型选择解析：run 已固定的冻结值 > 持久化意图 > 目标 Host 首选；首次派发成功后固化。
    const existingRun = await repo.getRun(request.runId);
    let resolvedSelection: ModelSelection;
    const fixedSelection = existingRun?.modelSelection;
    if (fixedSelection) {
      resolvedSelection = fixedSelection;
    } else {
      const persistedSelection = await repo.getModelSelectionForDispatch(
        request.automationId,
        request.workspaceKey,
      );
      if (persistedSelection) {
        const view = await modelSelectionService.getView({ selection: persistedSelection });
        if (view.selectionIssue || !view.effectiveSelection) {
          throw new Error("Automation 模型选择不可用，请重新选择模型与思考档位");
        }
        resolvedSelection = view.effectiveSelection;
      } else {
        const preferred = (await modelSelectionService.getView()).preferredSelection;
        if (!preferred) {
          throw new Error("Automation 无法从目标 Host 解析首选模型");
        }
        resolvedSelection = preferred;
      }
      resolvedSelection = await repo.fixRunModelSelection(request.runId, resolvedSelection);
    }
    const thoughtLevel = resolvedSelection.options?.reasoningLevel;

    // targetTaskId 存在＝会话内创建的 automation，追加回绑定会话；否则新建 task。
    let taskId: string;
    if (request.targetTaskId) {
      taskId = request.targetTaskId;
      await modeTaskService.resumeTask({
        taskId,
        workspacePath: request.workspacePath,
        ...(request.workspaceIdentity ? { workspaceIdentity: request.workspaceIdentity } : {}),
        model: formatModelPickerValue(resolvedSelection),
        ...(thoughtLevel ? { thoughtLevel } : {}),
        automationId: request.automationId,
      });
      await modeTaskService.setAutomationSessionConfig({
        taskId,
        traceId: request.runId as TraceId,
        modelSelection: resolvedSelection,
        ...(thoughtLevel ? { thoughtLevel } : {}),
        ...(request.mode ? { mode: request.mode } : {}),
      });
    } else {
      const created = await modeTaskService.createTask({
        workspacePath: request.workspacePath,
        ...(request.workspaceIdentity ? { workspaceIdentity: request.workspaceIdentity } : {}),
        modelSelection: resolvedSelection,
        ...(request.mode ? { mode: request.mode } : {}),
        automationId: request.automationId,
      });
      taskId = created.taskId;
    }

    // Bot 回推是 best-effort 辅助通道：订阅失败不能阻断派发与结算。
    const botsService = targetServices.getOptional(IBotsService);
    if (botsService) {
      try {
        const deliveryTarget = await repo.getBotDeliveryTarget(
          request.automationId,
          request.workspaceKey,
        );
        if (deliveryTarget) {
          await botsService.watchAutomationRun({
            target: deliveryTarget,
            taskId,
            workspacePath: request.workspacePath,
            ...(request.workspaceIdentity ? { workspaceIdentity: request.workspaceIdentity } : {}),
          });
        }
      } catch (error) {
        log(
          "warn",
          `automation Bot delivery 订阅失败 automation=${request.automationId}: ${describeError(error)}`,
        );
      }
    }

    // 终态订阅必须先于 sendPrompt 注册，快速完成的 turn 也不能漏回写。
    trackRunOutcome(targetServices, request, taskId);

    try {
      await modeTaskService.sendPrompt({
        taskId,
        traceId: request.runId as TraceId,
        content: request.prompt,
        clientMode: "desktop-continuous",
        automationId: request.automationId,
      });
    } catch (error) {
      // prompt 未被接受：终态永远不会来，就地退订并停掉心跳，避免订阅泄漏。
      const tracking = activeTrackings.get(request.runId);
      tracking?.disposeHeartbeat();
      activeTrackings.delete(request.runId);
      tracking?.disposeSubscription();
      throw error;
    }
    return { taskId, sessionId: taskId };
  }

  /** schedule 派发成功/失败的台账结算：推进 next_run_at 或写退避重试。 */
  async function settleScheduledDispatch(
    automation: ModeAutomation,
    request: AutomationDispatchRequest,
    outcome: { ok: true; result: AutomationDispatchResult } | { ok: false; error: string },
  ): Promise<void> {
    inFlightScheduleClaims.delete(request.automationId);
    const now = Date.now();
    if (outcome.ok) {
      try {
        await repo.markRunDispatch({
          runId: request.runId,
          dispatchStatus: "dispatched",
          sessionId: outcome.result.sessionId,
        });
        const fresh = await repo.get(request.automationId);
        const nextRunAt = fresh ? computeAutomationNextRunAt(fresh, now) : null;
        await repo.markDispatched(request.automationId, { dispatchedAt: now, nextRunAt });
      } catch (error) {
        log(
          "error",
          `结算定时任务派发失败 automation=${request.automationId}: ${describeError(error)}`,
        );
      }
      return;
    }
    try {
      await repo.markRunDispatch({
        runId: request.runId,
        dispatchStatus: "failed_to_dispatch",
        error: outcome.error,
      });
      const fresh = await repo.get(request.automationId);
      await repo.markDispatchFailed(request.automationId, {
        failedAt: now,
        error: outcome.error,
        // 确定性配置错误重试不会自愈，但与 Electron 线行为保持一致：统一按 transient 退避，
        // 由 DISPATCH_MAX_ATTEMPTS 上限收敛。
        kind: "transient",
        nextRunAt: fresh ? computeAutomationNextRunAt(fresh, now) : null,
      });
    } catch (error) {
      log(
        "error",
        `结算定时任务派发失败 automation=${request.automationId}: ${describeError(error)}`,
      );
    }
  }

  async function handleClaimedSchedule(automation: ModeAutomation, now: number): Promise<void> {
    const scheduledAt = automation.nextRunAt ?? automation.retryAt ?? now;
    const runId = buildScheduledRunId(automation.automationId, scheduledAt);
    const workspaceKey = resolveWorkspaceKey({
      workspacePath: automation.workspacePath,
      ...(automation.workspaceIdentity ? { workspaceIdentity: automation.workspaceIdentity } : {}),
    });
    const isRetry = automation.dispatchAttempts > 0;

    // misfire：首轮且计划触发时间已远早于 now → 认定错过窗口，跳过不补跑。
    const missed =
      !isRetry && automation.nextRunAt != null && automation.nextRunAt <= now - misfireGraceMs;
    if (missed) {
      // 纯一次性任务错过即终态，不得再从兼容性 scheduleRule 推导后续周期。
      const finalize = isOneShotAutomation(automation);
      const nextRunAt = finalize ? null : computeAutomationNextRunAt(automation, now);
      await repo.skipAndReschedule({
        automationId: automation.automationId,
        runId,
        workspaceKey,
        scheduledAt,
        reason: "computer_asleep_or_app_not_running",
        nextRunAt,
        finalize,
      });
      log(
        "info",
        `skip missed window automation=${automation.automationId} scheduledAt=${scheduledAt}${finalize ? " finalized=one-shot" : ""}`,
      );
      return;
    }

    await repo.upsertRunClaimed({
      runId,
      automationId: automation.automationId,
      workspaceKey,
      scheduledAt,
      trigger: "schedule",
    });
    inFlightScheduleClaims.add(automation.automationId);

    const request: AutomationDispatchRequest = {
      automationId: automation.automationId,
      runId,
      prompt: automation.prompt,
      ...(automation.targetTaskId ? { targetTaskId: automation.targetTaskId } : {}),
      ...(automation.modelSelection ? { modelSelection: automation.modelSelection } : {}),
      ...(automation.mode ? { mode: automation.mode } : {}),
      workspacePath: automation.workspacePath,
      ...(automation.workspaceIdentity ? { workspaceIdentity: automation.workspaceIdentity } : {}),
      workspaceKey,
      trigger: "schedule",
      scheduledAt,
    };

    try {
      const result = await dispatchRun(request);
      await settleScheduledDispatch(automation, request, { ok: true, result });
      log(
        "info",
        `automation dispatched automation=${automation.automationId} runId=${runId} taskId=${result.taskId}`,
      );
    } catch (error) {
      await settleScheduledDispatch(automation, request, {
        ok: false,
        error: describeError(error),
      });
      log(
        "warn",
        `automation dispatch failed automation=${automation.automationId} runId=${runId}: ${describeError(error)}`,
      );
    }
  }

  /** claimManualRuns 命中的是直接派发崩溃后遗留的 manual run（attempts=0 或超时）。 */
  async function handleClaimedManualRecovery(
    automation: ModeAutomation,
    run: ModeAutomationRun,
  ): Promise<void> {
    const workspaceKey = run.workspaceKey;
    const request: AutomationDispatchRequest = {
      automationId: automation.automationId,
      runId: run.runId,
      prompt: automation.prompt,
      ...(automation.targetTaskId ? { targetTaskId: automation.targetTaskId } : {}),
      ...((run.modelSelection ?? automation.modelSelection)
        ? { modelSelection: run.modelSelection ?? automation.modelSelection }
        : {}),
      ...(automation.mode ? { mode: automation.mode } : {}),
      workspacePath: automation.workspacePath,
      ...(automation.workspaceIdentity ? { workspaceIdentity: automation.workspaceIdentity } : {}),
      workspaceKey,
      trigger: "manual",
      scheduledAt: run.scheduledAt ?? null,
    };
    let result: AutomationDispatchResult;
    try {
      result = await dispatchRun(request);
    } catch (error) {
      log(
        "warn",
        `manual run recovery dispatch failed runId=${run.runId}: ${describeError(error)}`,
      );
      try {
        await repo.markRunDispatch({
          runId: run.runId,
          dispatchStatus: "failed_to_dispatch",
          error: describeError(error),
        });
        await repo.releaseManualClaim(automation.automationId, workspaceKey);
      } catch (settleError) {
        log(
          "error",
          `结算 manual run 恢复派发失败 runId=${run.runId}: ${describeError(settleError)}`,
        );
      }
      return;
    }
    // prompt 已 accepted：台账回写失败不能伪装成派发失败并提前释放锁，真实终态仍由
    // 终态订阅收口（releaseManualClaim）。
    try {
      await repo.markManualRunDispatched({
        runId: run.runId,
        sessionId: result.sessionId,
        dispatchedAt: Date.now(),
      });
      log("info", `manual run recovery dispatched runId=${run.runId} taskId=${result.taskId}`);
    } catch (error) {
      log("warn", `回写 manual run 派发台账失败 runId=${run.runId}: ${describeError(error)}`);
    }
  }

  /** 当前在途 tick 的 promise；wake() 等待它完成，保证「唤醒即结算」可观测。 */
  let currentTick: Promise<void> | null = null;

  async function tick(): Promise<void> {
    if (disposed || ticking) return;
    ticking = true;
    try {
      do {
        tickRequested = false;
        try {
          const now = Date.now();
          const claimed = await repo.claimDue(now);
          for (const automation of claimed) {
            await handleClaimedSchedule(automation, now);
          }
          const manualRuns = await repo.claimManualRuns(now);
          for (const { automation, run } of manualRuns) {
            await handleClaimedManualRecovery(automation, run);
          }
        } catch (error) {
          log("error", `automation tick failed: ${describeError(error)}`);
        }
      } while (tickRequested && !disposed);
    } finally {
      ticking = false;
    }
  }

  function requestTick(): Promise<void> {
    if (disposed) return Promise.resolve();
    if (currentTick) {
      tickRequested = true;
      return currentTick;
    }
    currentTick = tick().finally(() => {
      currentTick = null;
    });
    return currentTick;
  }

  return {
    start(): void {
      if (disposed || started) return;
      started = true;
      pollTimer = setInterval(() => void requestTick(), pollIntervalMs);
      void requestTick();
      log("info", `automation runtime started pollIntervalMs=${pollIntervalMs}`);
    },

    async stop(): Promise<void> {
      if (disposed) return;
      disposed = true;
      if (pollTimer) clearInterval(pollTimer);
      pollTimer = null;
      // 等在途 tick 结算完成：tick 内可能刚注册终态订阅，先让它收尾再统一退订，
      // 否则这些订阅会逃过 stop 清理而泄漏。
      if (currentTick) {
        try {
          await currentTick;
        } catch {
          // tick 内部已按轮捕获错误，这里只是防御。
        }
      }
      // 退订全部终态订阅并释放本进程仍持有的认领，避免下次启动等到 CLAIM_STALE 才回收。
      for (const tracking of activeTrackings.values()) {
        tracking.disposeHeartbeat();
        tracking.disposeSubscription();
        try {
          if (tracking.trigger === "manual") {
            await repo.releaseManualClaim(tracking.automationId, tracking.workspaceKey);
          } else {
            await repo.releaseClaim(tracking.automationId);
          }
        } catch {
          // 退出路径尽力而为。
        }
      }
      activeTrackings.clear();
      for (const automationId of inFlightScheduleClaims) {
        try {
          await repo.releaseClaim(automationId);
        } catch {
          // 退出路径尽力而为。
        }
      }
      inFlightScheduleClaims.clear();
      log("info", "automation runtime stopped");
    },

    wake(): Promise<void> {
      return requestTick();
    },

    async dispatchManualRun(params): Promise<void> {
      if (disposed) {
        throw new Error("Automation runtime is disposed.");
      }
      const { automation, run } = params;
      const workspaceKey = run.workspaceKey;
      const request: AutomationDispatchRequest = {
        automationId: automation.automationId,
        runId: run.runId,
        prompt: automation.prompt,
        ...(automation.targetTaskId ? { targetTaskId: automation.targetTaskId } : {}),
        ...((run.modelSelection ?? automation.modelSelection)
          ? { modelSelection: run.modelSelection ?? automation.modelSelection }
          : {}),
        ...(automation.mode ? { mode: automation.mode } : {}),
        workspacePath: automation.workspacePath,
        ...(automation.workspaceIdentity
          ? { workspaceIdentity: automation.workspaceIdentity }
          : {}),
        workspaceKey,
        trigger: "manual",
        scheduledAt: run.scheduledAt ?? null,
      };
      try {
        let result: AutomationDispatchResult;
        try {
          result = await dispatchRun(request);
        } catch (error) {
          log(
            "warn",
            `manual automation dispatch failed automation=${automation.automationId} runId=${run.runId}: ${describeError(error)}`,
          );
          try {
            await repo.markRunDispatch({
              runId: run.runId,
              dispatchStatus: "failed_to_dispatch",
              error: describeError(error),
            });
            await repo.releaseManualClaim(automation.automationId, workspaceKey);
          } catch (settleError) {
            log(
              "error",
              `结算 manual automation 派发失败 runId=${run.runId}: ${describeError(settleError)}`,
            );
          }
          // rethrow：modeAgentService 把错误透传给 UI，展示「触发运行失败」。
          throw error;
        }
        // prompt 已 accepted：台账回写失败不能伪装成派发失败并提前释放锁（否则 UI 误报失败、
        // turn 在途却丢了 single-flight），真实终态仍由终态订阅收口。
        try {
          await repo.markManualRunDispatched({
            runId: run.runId,
            sessionId: result.sessionId,
            dispatchedAt: Date.now(),
          });
          log(
            "info",
            `manual automation dispatched automation=${automation.automationId} runId=${run.runId} taskId=${result.taskId}`,
          );
        } catch (error) {
          log(
            "warn",
            `回写 manual automation 派发台账失败 runId=${run.runId}: ${describeError(error)}`,
          );
        }
      } finally {
        // 直派已结算或已失败；唤醒轮询补认领同 workspace 的到期 schedule 任务。
        void requestTick();
      }
    },
  };
}
