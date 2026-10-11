import { useEffect } from "react";
import { isRemoteWorkspaceIdentity } from "@mode/shared";
import type { CuaAccessibilitySettingsResult, CuaPermissionKind } from "@mode/shared";
import { requiredCuaPermissionsForRequestAccessStatus } from "@mode/shared/mode-protocol-v4";
import {
  isCuaPermissionStatusAvailable,
  type CuaPermissionRestartOptions,
  type CuaPermissionStatus,
  type CuaPermissionStatusResult,
  type ModeAgentCuaPermissionObservation,
} from "@mode/services";
import { resolveCuaPermissionSource } from "@/cua-permission/cuaPermissionObservationSource.js";
import { useConfirmDialog } from "@/hooks/useConfirmDialog.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { useServices } from "@/hooks/useServices.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { shouldRestartHelperAfterCuaPermissionReturn } from "@/lib/cuaPermissionAction.js";
import { createCuaPermissionOnboardingOperationId } from "@/lib/cuaPermissionOnboardingOperation.js";
import { fetchCuaPermissionStatus } from "@/lib/cuaPermissionStatusStore.js";
import { logger } from "@/logger.js";
import { isRemoteCuaPermissionRowVisible } from "@/settings/computerUseAvailability.js";
import { requiredCuaPermissionsForFreshStatus } from "@/settings/cuaPermissionPreparation.js";
import {
  useRemoteWorkspaceSessionStore,
  type RemoteWorkspaceSession,
} from "@/store/remoteWorkspaceSessionStore.js";

interface CuaPermissionPromptDependencies {
  getStatus(): Promise<CuaPermissionStatusResult>;
  confirm(required: CuaPermissionKind[]): Promise<boolean>;
  openOnboarding(required: CuaPermissionKind[]): Promise<CuaAccessibilitySettingsResult>;
  restartHelper(options: CuaPermissionRestartOptions): Promise<{ ok: boolean; reason?: string }>;
  refresh(): void;
  isDisposed?(): boolean;
  /**
   * pane 分谓词（可选，缺省回落本机既有 available===true 口径）：对端 getStatus 成功回包
   * 不携带 available 字段，沿用本机谓词会让对端权限卡永不出现；对端来源须传
   * isRemoteCuaPermissionRowVisible（available!==false，T4 ledger 裁定的远端谓词）。
   * docs/specs/lan-remote-computer-use.md 事件顺序 ④ 验收「本地对话出现权限观察卡片」。
   */
  isStatusUsable?(status: CuaPermissionStatusResult): status is CuaPermissionStatus;
}

const CUA_PERMISSION_RETRY_DELAY_MS = 500;

function assertCuaPermissionPromptActive(dependencies: CuaPermissionPromptDependencies): void {
  if (dependencies.isDisposed?.()) throw new Error("CUA permission prompt disposed");
}

async function runWithOneRetry<T>(
  dependencies: CuaPermissionPromptDependencies,
  operation: () => Promise<T>,
  failureReason?: (result: T) => string | undefined,
): Promise<T> {
  for (let attempt = 0; attempt < 2; attempt += 1) {
    assertCuaPermissionPromptActive(dependencies);
    try {
      const result = await operation();
      assertCuaPermissionPromptActive(dependencies);
      const reason = failureReason?.(result);
      if (reason) throw new Error(reason);
      return result;
    } catch (error) {
      if (attempt === 1 || dependencies.isDisposed?.()) throw error;
      await new Promise((resolve) => setTimeout(resolve, CUA_PERMISSION_RETRY_DELAY_MS));
    }
  }
  throw new Error("CUA permission retry exhausted");
}

function cuaPermissionObservationKey(observation: ModeAgentCuaPermissionObservation): string {
  const workspaceKey = observation.workspaceIdentity?.trim() || observation.workspacePath;
  const missing = requiredCuaPermissionsForRequestAccessStatus(observation.permissionStatus);
  return [
    workspaceKey,
    observation.sessionId,
    observation.turnId ?? observation.eventId,
    missing.join(","),
  ].join("\0");
}

async function runCuaPermissionPrompt(
  dependencies: CuaPermissionPromptDependencies,
): Promise<void> {
  const readMissing = async () => {
    const status = await runWithOneRetry(dependencies, dependencies.getStatus);
    // pane 分谓词（docs/specs/lan-remote-computer-use.md 事件顺序 ④）：base 来源沿用既有
    // available===true 口径（本机行为与基线逐字一致，T4 ledger 裁定本机消费方不动）；对端来源
    // 用 available!==false——对端 getStatus 成功回包不携带 available 字段（见 node.ts 成功形态），
    // 沿用本机谓词会返回空缺失列表，对端权限卡永不出现。
    const isUsable: (value: CuaPermissionStatusResult) => value is CuaPermissionStatus =
      dependencies.isStatusUsable ?? isCuaPermissionStatusAvailable;
    return isUsable(status) ? requiredCuaPermissionsForFreshStatus(status) : [];
  };
  const beforeConfirm = await readMissing();
  if (beforeConfirm.length === 0) return;
  assertCuaPermissionPromptActive(dependencies);
  if (!(await dependencies.confirm(beforeConfirm))) return;
  assertCuaPermissionPromptActive(dependencies);

  // 原因：确认框停留期间可能已从另一窗口授权，打开设置前必须再读一次 Helper 真值。
  const afterConfirm = await readMissing();
  if (afterConfirm.length === 0) return;
  const result = await runWithOneRetry(
    dependencies,
    () => dependencies.openOnboarding(afterConfirm),
    (value) =>
      !value.success && !value.canceled
        ? `CUA permission onboarding failed: ${value.error ?? "unknown"}`
        : undefined,
  );
  if (result.canceled) return;
  if (shouldRestartHelperAfterCuaPermissionReturn(result)) {
    // 原因：Helper 重启失败时只能重试重启本身，重复打开系统设置会打断用户操作。
    await runWithOneRetry(
      dependencies,
      () =>
        dependencies.restartHelper({
          reason: "permission_granted",
          ...(result.sessionId ? { onboardingSessionId: result.sessionId } : {}),
        }),
      (value) => (value.ok ? undefined : `CUA Helper restart failed: ${value.reason ?? "unknown"}`),
    );
  }
  if (result.success && result.returnedFromSettings) {
    assertCuaPermissionPromptActive(dependencies);
    dependencies.refresh();
  }
}

export function CuaPermissionObservationAttachment() {
  const services = useServices();
  const platform = usePlatform();
  const confirmDialog = useConfirmDialog();
  const { intl } = useModeIntl();

  useEffect(() => {
    const handled = new Set<string>();
    const pending = new Set<string>();
    let chain = Promise.resolve();
    let activeOperationId: string | null = null;
    let disposed = false;

    // 观察按来源 accessor 归属（docs/specs/lan-remote-computer-use.md 接口变更 5、事件顺序 ④）：
    // workspaceIdentity 解析到在册 remote session 时，状态/重启/刷新/引导全部走对端服务；
    // 本机观察维持 base。handleObservation 对 base 与每个在册 remote session 的订阅共用。
    const handleObservation = (observation: ModeAgentCuaPermissionObservation) => {
      const missing = requiredCuaPermissionsForRequestAccessStatus(observation.permissionStatus);
      const key = cuaPermissionObservationKey(observation);
      if (disposed || missing.length === 0 || handled.has(key) || pending.has(key)) return;
      const storeState = useRemoteWorkspaceSessionStore.getState();
      const source = resolveCuaPermissionSource({
        baseServices: services,
        observation,
        sessionsById: storeState.sessionsById,
        sessionIdByWorkspaceIdentity: storeState.sessionIdByWorkspaceIdentity,
      });
      // 对端 identity 的观察却解析不到在册能力 session（注销间隙 / ssh-wsl-docker 会话）时
      // fail closed：既不查本机状态，更不许把对端引导开到本机（spec ④「绝不打开本机的」）。
      const identity = observation.workspaceIdentity?.trim();
      if (!source.remote && identity && isRemoteWorkspaceIdentity(identity)) return;
      const permissionService = source.services.cuaPermissionService;
      if (!permissionService) return;
      pending.add(key);

      chain = chain.then(async () => {
        if (disposed) return;
        try {
          await runCuaPermissionPrompt({
            getStatus: () =>
              permissionService.getStatus(
                observation.workspacePath,
                observation.workspaceIdentity,
                { includeFunctionalProbes: false },
              ),
            confirm: (required) =>
              confirmDialog({
                title: intl.formatMessage({ id: "cuaPermission.live.title" }),
                // 描述文案按来源分流：对端引导要显式说明「引导开在对端设备、在对端完成授权」
                // （docs/specs/lan-remote-computer-use.md UI 规则 i18n 行）；本机维持既有文案。
                description: source.remote
                  ? intl.formatMessage(
                      { id: "cua.permission.remoteOnboardingDescription" },
                      {
                        permissions: required
                          .map((kind) =>
                            intl.formatMessage({
                              id:
                                kind === "accessibility"
                                  ? "cuaPermission.perm.accessibility"
                                  : "cuaPermission.perm.screenRecording",
                            }),
                          )
                          .join(", "),
                      },
                    )
                  : intl.formatMessage({ id: "cuaPermission.live.description" }),
                confirmLabel: intl.formatMessage({ id: "cuaPermission.live.confirm" }),
                cancelLabel: intl.formatMessage({ id: "cuaPermission.live.cancel" }),
              }),
            openOnboarding: async (required) => {
              if (source.remote) {
                // 对端引导经 T7 的 ICuaPermissionService.openPermissionOnboarding 透传，
                // 打开的是对端机器的引导窗口（spec ④）；对端分支不产生 operationId，
                // cleanup 的 cancelCuaPermissionOnboarding 只作用于本机分支。
                const service = source.services.cuaPermissionService;
                if (!service?.openPermissionOnboarding) {
                  return { success: false, error: "unavailable" };
                }
                return (
                  (await service.openPermissionOnboarding(
                    observation.workspacePath,
                    observation.workspaceIdentity,
                    {
                      initialPermission: required[0],
                      requiredPermissions: required,
                    },
                  )) ?? { success: false, error: "unavailable" }
                );
              }
              const operationId = createCuaPermissionOnboardingOperationId();
              activeOperationId = operationId;
              try {
                return (
                  (await platform.openCuaPermissionOnboarding?.({
                    operationId,
                    initialPermission: required[0],
                    requiredPermissions: required,
                  })) ?? { success: false, error: "unavailable" }
                );
              } finally {
                if (activeOperationId === operationId) activeOperationId = null;
              }
            },
            restartHelper: (options) =>
              permissionService.restartHelper(
                observation.workspacePath,
                observation.workspaceIdentity,
                options,
              ),
            refresh: () =>
              fetchCuaPermissionStatus({
                service: permissionService,
                workspacePath: observation.workspacePath,
                workspaceIdentity: observation.workspaceIdentity,
                options: { includeFunctionalProbes: false },
              }),
            isDisposed: () => disposed,
            // 对端状态缺 available 字段 → 远端谓词；base 维持本机既有谓词（见 readMissing）。
            isStatusUsable: source.remote
              ? isRemoteCuaPermissionRowVisible
              : isCuaPermissionStatusAvailable,
          });
          if (disposed) return;
          // 原因：只有正常终态（包括用户取消）才能去重；异常必须允许同一 observation 再次处理。
          handled.add(key);
          if (handled.size > 2_000) {
            const oldest = handled.values().next().value;
            if (typeof oldest === "string") handled.delete(oldest);
          }
        } catch (error: unknown) {
          if (!disposed) logger.warn("CUA 权限引导失败", { error });
        } finally {
          pending.delete(key);
        }
      });
    };

    // 本机 base 订阅沿用既有门（本机权限服务 + 本机 preload 引导函数 + 观察事件面），
    // 但该门只约束 base 订阅本身、不再整体 return——否则无本机 onboarding 的平台连对端
    // 订阅都拿不到（Web 端 allowRemoteWorkspace=false 无远端 pane，行为不受影响）。
    const baseSubscription =
      services.cuaPermissionService &&
      platform.openCuaPermissionOnboarding &&
      typeof services.modeAgentService.onDynamicCuaPermissionObservation === "function"
        ? services.modeAgentService.onDynamicCuaPermissionObservation()(handleObservation)
        : null;

    // 对端订阅集合：随 store 在册 session 差量增删；同 sessionId 换代（session 对象引用变化）
    // 时旧 services 退订、新代重订。对端门只要求 session 自带权限服务 + 观察事件面，
    // 不要求本机 preload 函数（那是本机 preload，对端用不上）。
    const remoteSubscriptions = new Map<
      string,
      { session: RemoteWorkspaceSession; subscription: { dispose(): void } }
    >();
    const syncRemoteSubscriptions = (): void => {
      if (disposed) return;
      const sessionsById = useRemoteWorkspaceSessionStore.getState().sessionsById;
      // Map 迭代中删除当前 key 是安全的（只删正在访问的项，未访问项不受影响）。
      for (const [sessionId, entry] of remoteSubscriptions) {
        if (sessionsById[sessionId] === entry.session) continue;
        entry.subscription.dispose();
        remoteSubscriptions.delete(sessionId);
      }
      for (const [sessionId, session] of Object.entries(sessionsById)) {
        if (remoteSubscriptions.has(sessionId)) continue;
        if (!session.services.cuaPermissionService) continue;
        if (typeof session.services.modeAgentService.onDynamicCuaPermissionObservation !== "function")
          continue;
        remoteSubscriptions.set(sessionId, {
          session,
          subscription: session.services.modeAgentService.onDynamicCuaPermissionObservation()(
            handleObservation,
          ),
        });
      }
    };
    const unsubscribeFromSessionStore = useRemoteWorkspaceSessionStore.subscribe(
      syncRemoteSubscriptions,
    );
    syncRemoteSubscriptions();

    return () => {
      // 原因：先封闭生命周期边界，避免已排队任务在 subscription 释放后继续触发旧 workspace 副作用。
      disposed = true;
      unsubscribeFromSessionStore();
      baseSubscription?.dispose();
      for (const entry of remoteSubscriptions.values()) entry.subscription.dispose();
      remoteSubscriptions.clear();
      if (activeOperationId) platform.cancelCuaPermissionOnboarding?.(activeOperationId);
    };
  }, [confirmDialog, intl, platform, services]);

  return null;
}
