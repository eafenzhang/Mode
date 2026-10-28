/**
 * fnOS fpk 「应用更新」卡片（docs/specs/fnos-feature-parity-and-update.md）。
 *
 * 只在平台实现 `getFnosAppUpdateState` 且服务端返回 enabled 时渲染；桌面端没有
 * 这些方法，普通 Web 部署返回 enabled=false，两种情况都不显示。
 * 状态唯一所有者是服务端 FnosAppUpdater；这里只轮询投影并发送命令。
 */
import { useCallback, useEffect, useState } from "react";
import type { FnosAppUpdateState } from "@mode/shared";
import { Button } from "@/ui-foundation/adapters/button.js";
import { useConfirmDialog } from "@/hooks/useConfirmDialog.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { SettingsRow } from "@/settings/SettingsPageParts.js";
import { logger } from "@/logger.js";

const ACTIVE_PHASES = new Set(["checking", "downloading", "applying"]);

function isActivePhase(state: FnosAppUpdateState | null): boolean {
  return state !== null && ACTIVE_PHASES.has(state.phase);
}

export function FnosAppUpdateCard() {
  const platform = usePlatform();
  const { intl } = useModeIntl();
  const requestConfirmation = useConfirmDialog();
  const [state, setState] = useState<FnosAppUpdateState | null>(null);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);

  const loadState = useCallback(async () => {
    if (!platform.getFnosAppUpdateState) {
      return;
    }
    try {
      setState(await platform.getFnosAppUpdateState());
    } catch (error) {
      logger.warn("[FnosAppUpdateCard] 读取更新状态失败", { error });
      setActionError(error instanceof Error ? error.message : String(error));
    }
  }, [platform]);

  useEffect(() => {
    void loadState();
  }, [loadState]);

  useEffect(() => {
    if (!isActivePhase(state) || !platform.getFnosAppUpdateState) {
      return;
    }
    const timer = globalThis.setInterval(() => {
      void loadState();
    }, 2000);
    return () => globalThis.clearInterval(timer);
  }, [loadState, platform, state]);

  if (!platform.getFnosAppUpdateState) {
    return null;
  }
  if (state === null) {
    return null;
  }
  if (!state.enabled) {
    return null;
  }

  const runAction = async (action: () => Promise<FnosAppUpdateState | void>) => {
    if (busy) {
      return;
    }
    setBusy(true);
    setActionError(null);
    try {
      const next = await action();
      if (next) {
        setState(next);
      }
    } catch (error) {
      logger.warn("[FnosAppUpdateCard] 更新操作失败", { error });
      setActionError(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  const handleApply = async () => {
    if (!platform.applyFnosAppUpdate) {
      return;
    }
    const version = state.latestVersion ?? "";
    const confirmed = await requestConfirmation({
      title: intl.formatMessage({ id: "settings.fnosAppUpdate.confirm.title" }, { version }),
      description: intl.formatMessage({ id: "settings.fnosAppUpdate.confirm.description" }),
      confirmLabel: intl.formatMessage({ id: "settings.fnosAppUpdate.confirm.ok" }),
      cancelLabel: intl.formatMessage({ id: "settings.fnosAppUpdate.confirm.cancel" }),
    });
    if (!confirmed) {
      return;
    }
    setBusy(true);
    setActionError(null);
    try {
      const result = await platform.applyFnosAppUpdate();
      if (!result.ok) {
        setActionError(result.error ?? "apply_failed");
        return;
      }
      // 服务端会在约 0.5s 后执行 cmd/main restart；stop 最坏 SIGTERM→10s→SIGKILL，
      // 再留 2s 启动余量后刷新，避免在服务还没起来时 reload 到错误页。
      if (typeof window !== "undefined") {
        globalThis.setTimeout(() => window.location.reload(), 12_000);
      }
    } catch (error) {
      logger.warn("[FnosAppUpdateCard] 应用更新失败", { error });
      setActionError(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  const statusText = (() => {
    switch (state.phase) {
      case "checking":
        return intl.formatMessage({ id: "settings.fnosAppUpdate.checking" });
      case "up-to-date":
        return intl.formatMessage({ id: "settings.fnosAppUpdate.upToDate" });
      case "available":
        return intl.formatMessage(
          { id: "settings.fnosAppUpdate.available" },
          { version: state.latestVersion ?? "" },
        );
      case "downloading":
        return intl.formatMessage(
          { id: "settings.fnosAppUpdate.downloading" },
          { progress: String(state.progress ?? 0) },
        );
      case "ready":
        return intl.formatMessage(
          { id: "settings.fnosAppUpdate.ready" },
          { version: state.latestVersion ?? "" },
        );
      case "applying":
        return intl.formatMessage({ id: "settings.fnosAppUpdate.applying" });
      case "error":
        return intl.formatMessage(
          { id: "settings.fnosAppUpdate.error" },
          { error: state.error ?? "unknown" },
        );
      case "idle":
      default:
        return "";
    }
  })();

  const progressPercent =
    state.phase === "downloading" ? Math.max(0, Math.min(100, state.progress ?? 0)) : 0;
  const disabled = busy || isActivePhase(state);

  return (
    <SettingsRow
      label={intl.formatMessage({ id: "settings.fnosAppUpdate.title" })}
      description={
        <>
          <span>
            {intl.formatMessage(
              { id: "settings.fnosAppUpdate.description" },
              { current: state.currentVersion },
            )}
          </span>
          {statusText ? <span className="mt-1 block">{statusText}</span> : null}
          {actionError ? <span className="mt-1 block text-destructive">{actionError}</span> : null}
          {progressPercent > 0 ? (
            <span className="mt-2 block h-1.5 w-full overflow-hidden rounded-full bg-surface">
              <span
                className="block h-full rounded-full bg-foreground-subtle transition-[width]"
                style={{ width: `${String(progressPercent)}%` }}
              />
            </span>
          ) : null}
        </>
      }
      control={
        state.phase === "ready" ? (
          <Button
            type="button"
            variant="secondary"
            size="sm"
            disabled={disabled}
            onClick={() => void handleApply()}
          >
            {intl.formatMessage({ id: "settings.fnosAppUpdate.apply" })}
          </Button>
        ) : state.phase === "available" ? (
          <Button
            type="button"
            size="sm"
            disabled={disabled}
            onClick={() =>
              void runAction(async () => {
                const next = await platform.downloadFnosAppUpdate?.();
                return next;
              })
            }
          >
            {intl.formatMessage({ id: "settings.fnosAppUpdate.download" })}
          </Button>
        ) : (
          <Button
            type="button"
            variant="secondary"
            size="sm"
            disabled={disabled}
            onClick={() =>
              void runAction(async () => {
                const next = await platform.checkFnosAppUpdate?.();
                return next;
              })
            }
          >
            {intl.formatMessage({ id: "settings.fnosAppUpdate.check" })}
          </Button>
        )
      }
    />
  );
}
