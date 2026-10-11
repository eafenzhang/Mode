/**
 * 设置页「远程桌面」分区（被控端视角，docs/specs/lan-remote-desktop.md 产品规则 1/2 与 UI 规则）：
 * - 被控总开关：默认关；关 → 连接请求直接拒绝且进行中的会话立即终止（main 侧收口）；
 * - 已授权设备（同意弹窗「记住此设备」写入）：逐个撤销，撤销后该设备下次连接重新弹窗；
 * - 只读写 main 经 host 现读的被控设置（唯一属主），本组件不缓存第二份配置。
 *
 * 入口只在 win32 桌面端出现（settingsPageConfig 门），因此这里直接调用平台 rd* 方法。
 */
import { useCallback, useEffect, useState } from "react";
import { LoaderIcon, MonitorSmartphone, RefreshCw, ShieldCheck, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button.js";
import { Switch } from "@/components/ui/switch.js";
import { toast } from "@/components/ui/toast.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { SettingsGroupCard, SettingsRow } from "@/settings/SettingsPageParts.js";
import type { RemoteDesktopServerState } from "@/remote-desktop/platformTypes.js";

const EMPTY_STATE: RemoteDesktopServerState = { enabled: false, authorizedClients: [] };

export function RemoteDesktopSettingsSection() {
  const { intl } = useModeIntl();
  const platform = usePlatform();
  const [state, setState] = useState<RemoteDesktopServerState>(EMPTY_STATE);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);

  const applyState = (next: RemoteDesktopServerState) => {
    setState(next);
  };

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const next = await platform.rdGetServerState();
        if (!cancelled) {
          applyState(next);
        }
      } catch {
        // 读失败按 fail-closed 展示（关 + 空授权），写路径仍以 main 现读为准。
        if (!cancelled) {
          applyState(EMPTY_STATE);
        }
      } finally {
        if (!cancelled) {
          setLoading(false);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [platform]);

  const run = useCallback(
    async (action: () => Promise<RemoteDesktopServerState>, failureMessageId: string) => {
      setBusy(true);
      try {
        applyState(await action());
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        toast(`${intl.formatMessage({ id: failureMessageId })}：${message}`);
      } finally {
        setBusy(false);
      }
    },
    [intl],
  );

  return (
    <div className="space-y-4">
      <SettingsGroupCard>
        <SettingsRow
          label={intl.formatMessage({ id: "settings.remoteDesktop.enable" })}
          description={intl.formatMessage({
            id: state.enabled
              ? "settings.remoteDesktop.statusEnabled"
              : "settings.remoteDesktop.statusDisabled",
          })}
          control={
            loading ? (
              <LoaderIcon className="size-4 animate-spin text-foreground-subtle" />
            ) : (
              <Switch
                checked={state.enabled}
                disabled={busy}
                aria-label={intl.formatMessage({ id: "settings.remoteDesktop.enable" })}
                onCheckedChange={(checked) =>
                  void run(
                    () => platform.rdSetServerEnabled(checked),
                    "settings.remoteDesktop.toggleFailed",
                  )
                }
              />
            )
          }
        />
        <SettingsRow
          label={intl.formatMessage({ id: "settings.remoteDesktop.authorizedClients" })}
          description={
            state.authorizedClients.length === 0
              ? intl.formatMessage({ id: "settings.remoteDesktop.noClients" })
              : intl.formatMessage(
                  { id: "settings.remoteDesktop.clientCount" },
                  { count: String(state.authorizedClients.length) },
                )
          }
          detail={
            state.authorizedClients.length > 0 ? (
              <ul className="space-y-2" data-testid="remote-desktop-authorized-clients">
                {state.authorizedClients.map((client) => (
                  <li
                    key={client.id}
                    className="flex items-center justify-between gap-3 rounded-lg border border-border bg-card px-3 py-2"
                  >
                    <span className="min-w-0">
                      <span className="block truncate text-ui-base text-foreground">
                        {client.label || client.id}
                      </span>
                      <span className="block truncate text-ui-xs text-foreground-subtle">
                        {client.id}
                      </span>
                    </span>
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      disabled={busy}
                      aria-label={intl.formatMessage(
                        { id: "settings.remoteDesktop.revoke" },
                        { name: client.label || client.id },
                      )}
                      onClick={() =>
                        void run(
                          () => platform.rdRevokeClient(client.id),
                          "settings.remoteDesktop.revokeFailed",
                        )
                      }
                    >
                      <Trash2 className="size-4" />
                    </Button>
                  </li>
                ))}
              </ul>
            ) : undefined
          }
          control={
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={busy || loading}
              onClick={() =>
                void run(() => platform.rdGetServerState(), "settings.remoteDesktop.loadFailed")
              }
            >
              <RefreshCw className="size-4" />
              {intl.formatMessage({ id: "settings.remoteDesktop.refresh" })}
            </Button>
          }
        />
      </SettingsGroupCard>

      <div className="space-y-2 px-1 text-ui-base text-foreground-subtle">
        <p className="flex items-start gap-1.5">
          <ShieldCheck className="mt-0.5 size-3.5 shrink-0" />
          {intl.formatMessage({ id: "settings.remoteDesktop.securityHint" })}
        </p>
        <p className="flex items-start gap-1.5">
          <MonitorSmartphone className="mt-0.5 size-3.5 shrink-0" />
          {intl.formatMessage({ id: "settings.remoteDesktop.lanHint" })}
        </p>
      </div>
    </div>
  );
}
