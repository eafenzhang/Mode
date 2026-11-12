import { useCallback, useEffect, useState } from "react";
import { AlertTriangleIcon, Cpu, LoaderIcon, ShieldCheck } from "lucide-react";
import type { LanAccessState } from "@mode/shared";
import type { RemoteDesktopEncoderCapability } from "@mode/shared/remote-desktop";
import { Switch } from "@/ui-foundation/adapters/switch.js";
import { toast } from "@/ui-foundation/adapters/toast.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { SettingsGroupCard, SettingsRow } from "@/settings/SettingsPageParts.js";

const EMPTY_STATE: LanAccessState = {
  enabled: false,
  port: null,
  addresses: [],
  pairCode: null,
  clients: [],
};

/**
 * 设置页「远程控制」分区（docs/specs/remote-control-unified-pairing.md R1/R2）：
 * 产品统一到个人账号后，这里只保留两个开关——
 * - 允许局域网直连（`lanAccessEnabled`）：Host 才会向同账号设备上报局域网直连地址；
 * - 允许被远程控制（`rdEnabled`）：关掉会立即终止进行中的被控会话。
 * 配对码、已配对客户端清单、「我配对的对端」都随配对入口一起移除；授权来源是同账号目录，
 * 后端配对接口仅保留给 CLI / 旧客户端。
 */
export function RemoteControlSettingsSection() {
  const { intl } = useModeIntl();
  const platform = usePlatform();
  const [state, setState] = useState<LanAccessState>(EMPTY_STATE);
  const [rdEnabled, setRdEnabled] = useState(false);
  // unknown = 尚无真实会话观测；software = 已回退系统软件编码（能出画面但更耗 CPU）；
  // unavailable = 硬软两路都拿不到编码器（会话起不来）。
  const [encoderCapability, setEncoderCapability] =
    useState<RemoteDesktopEncoderCapability>("unknown");
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      // 两个开关各读各的；读失败按 fail-closed（关）展示，写路径仍以 main 现读为准。
      const [lanState, rdState] = await Promise.allSettled([
        platform.getLanAccessState(),
        platform.rdGetServerState(),
      ]);
      if (cancelled) {
        return;
      }
      if (lanState.status === "fulfilled") {
        setState(lanState.value);
      } else {
        setError(
          lanState.reason instanceof Error ? lanState.reason.message : String(lanState.reason),
        );
      }
      setRdEnabled(rdState.status === "fulfilled" ? rdState.value.enabled : false);
      setEncoderCapability(
        rdState.status === "fulfilled" ? (rdState.value.encoderCapability ?? "unknown") : "unknown",
      );
      setLoading(false);
    })();
    return () => {
      cancelled = true;
    };
  }, [platform]);

  /** 被控总开关（允许被远程控制）：关开关 main 会立即终止进行中的被控会话。 */
  const toggleRdEnabled = useCallback(
    async (enabled: boolean) => {
      setBusy(true);
      try {
        const next = await platform.rdSetServerEnabled(enabled);
        setRdEnabled(next.enabled);
        setEncoderCapability(next.encoderCapability ?? "unknown");
        setError("");
      } catch (caught) {
        const message = caught instanceof Error ? caught.message : String(caught);
        setError(message);
        toast(`${intl.formatMessage({ id: "settings.remoteDesktop.toggleFailed" })}：${message}`);
      } finally {
        setBusy(false);
      }
    },
    [intl, platform],
  );

  const toggleLanAccess = useCallback(
    async (enabled: boolean) => {
      setBusy(true);
      try {
        setState(await platform.setLanAccessEnabled(enabled));
        setError("");
      } catch (caught) {
        const message = caught instanceof Error ? caught.message : String(caught);
        setError(message);
        toast(`${intl.formatMessage({ id: "settings.lanAccess.toggleFailed" })}：${message}`);
      } finally {
        setBusy(false);
      }
    },
    [intl, platform],
  );

  const statusText = state.enabled
    ? intl.formatMessage(
        { id: "settings.lanAccess.statusEnabled" },
        {
          // 本机信息的展示归宿：全部监听地址（含 Tailscale）一次给全。
          address: state.addresses.length > 0 ? state.addresses.join("、") : `:${state.port ?? ""}`,
        },
      )
    : intl.formatMessage({ id: "settings.lanAccess.statusDisabled" });

  return (
    <div className="space-y-4">
      <SettingsGroupCard>
        <SettingsRow
          label={intl.formatMessage({ id: "settings.lanAccess.enable" })}
          description={statusText}
          control={
            loading ? (
              <LoaderIcon className="size-4 animate-spin text-foreground-subtle" />
            ) : (
              <Switch
                checked={state.enabled}
                disabled={busy}
                aria-label={intl.formatMessage({ id: "settings.lanAccess.enable" })}
                onCheckedChange={(checked) => void toggleLanAccess(checked)}
              />
            )
          }
        />
        <SettingsRow
          label={intl.formatMessage({ id: "settings.remoteDesktop.enable" })}
          description={intl.formatMessage({
            id: rdEnabled
              ? "settings.remoteDesktop.statusEnabled"
              : "settings.remoteDesktop.statusDisabled",
          })}
          control={
            loading ? (
              <LoaderIcon className="size-4 animate-spin text-foreground-subtle" />
            ) : (
              <Switch
                checked={rdEnabled}
                disabled={busy}
                aria-label={intl.formatMessage({ id: "settings.remoteDesktop.enable" })}
                onCheckedChange={(checked) => void toggleRdEnabled(checked)}
              />
            )
          }
        />
      </SettingsGroupCard>

      <div className="space-y-2 px-1 text-ui-base text-foreground-subtle">
        {encoderCapability === "unavailable" ? (
          <p
            data-testid="remote-desktop-encoder-unavailable"
            className="flex items-start gap-1.5 text-warning"
          >
            <AlertTriangleIcon className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
            {intl.formatMessage({ id: "settings.remoteDesktop.encoderUnavailable" })}
          </p>
        ) : null}
        {encoderCapability === "software" ? (
          <p data-testid="remote-desktop-encoder-software" className="flex items-start gap-1.5">
            <Cpu className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
            {intl.formatMessage({ id: "settings.remoteDesktop.encoderSoftwareFallback" })}
          </p>
        ) : null}
        <p className="flex items-start gap-1.5">
          <ShieldCheck className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
          {intl.formatMessage({ id: "settings.remoteDesktop.securityHint" })}
        </p>
        {state.enabled ? (
          <p className="flex items-start gap-1.5">
            <AlertTriangleIcon className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
            {intl.formatMessage({ id: "settings.lanAccess.firewallHint" })}
          </p>
        ) : null}
      </div>
      {error ? <p className="px-1 text-ui-base text-destructive">{error}</p> : null}
    </div>
  );
}
