import { useCallback, useEffect, useState } from "react";
import {
  AlertTriangleIcon,
  CheckIcon,
  Copy,
  LoaderIcon,
  MonitorSmartphone,
  Radar,
  RefreshCw,
  ShieldCheck,
} from "lucide-react";
import type { LanAccessState } from "@mode/shared";
import { Button } from "@/components/ui/button.js";
import { Switch } from "@/components/ui/switch.js";
import { toast } from "@/components/ui/toast.js";
import { cn } from "@/components/lib/utils.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { SettingsGroupCard, SettingsRow } from "@/settings/SettingsPageParts.js";
import { LanAccessPairedPeersCard } from "@/settings/LanAccessPairedPeersCard.js";
import { RemoteControlPairedClientsList } from "@/settings/RemoteControlPairedClientsList.js";
import { logger } from "@/logger.js";
import type { RemoteDesktopServerState } from "@/remote-desktop/platformTypes.js";

const EMPTY_LAN_STATE: LanAccessState = {
  enabled: false,
  port: null,
  addresses: [],
  pairCode: null,
  clients: [],
};

const EMPTY_RD_STATE: RemoteDesktopServerState = { sharingEnabled: false };

/** 系统 RDP 状态 → 展示档（spec 产品规则 4/8：只检测、不改系统设置，不做一键开启）。 */
type SystemRdpFacet = "enabled" | "disabled" | "homeEdition" | "unknown";

function resolveSystemRdpFacet(state: RemoteDesktopServerState): SystemRdpFacet {
  if (!state.systemRdp) {
    return "unknown";
  }
  if (state.systemRdp.editionOk === false) {
    return "homeEdition";
  }
  return state.systemRdp.enabled ? "enabled" : "disabled";
}

function formatRemaining(expiresAt: number, now: number): string {
  const remainingSeconds = Math.max(0, Math.round((expiresAt - now) / 1000));
  const minutes = Math.floor(remainingSeconds / 60);
  const seconds = remainingSeconds % 60;
  return `${minutes}:${String(seconds).padStart(2, "0")}`;
}

/**
 * 设置页「远程控制」分区（spec: docs/specs/remote-desktop-system-rdp.md 产品规则 4/5/6）：
 * - LAN 访问开关与配对码：不变；
 * - **提示性共享开关**（原「允许被远程控制」总开关）：UI 明写「不影响系统远程桌面」——
 *   它只决定本机是否作为可远控设备出现在对端设备页（能力位），不是安全闸门；
 * - **系统远程桌面状态行**：显示 `systemRdp.enabled` / `editionOk`，未启用时给可复制的开启指引；
 * - **逐设备授权开关已删除**：系统 RDP 下授权发生在 Windows 登录，应用没有执行点，保留即为假控制。
 */
export function RemoteControlSettingsSection() {
  const { intl } = useModeIntl();
  const platform = usePlatform();
  const [state, setState] = useState<LanAccessState>(EMPTY_LAN_STATE);
  const [rdState, setRdState] = useState<RemoteDesktopServerState>(EMPTY_RD_STATE);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [now, setNow] = useState(() => Date.now());

  const applyState = (next: LanAccessState) => {
    setState(next);
    setError("");
  };

  /** 只重读 LAN 状态（移除设备改的是记录本身，单列表行内状态要立刻跟上）。 */
  const reloadLanState = useCallback(async () => {
    const next = await platform.getLanAccessState();
    applyState(next);
  }, [platform]);

  const refresh = useCallback(async () => {
    setBusy(true);
    try {
      await reloadLanState();
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught);
      setError(message);
      toast(`${intl.formatMessage({ id: "settings.remoteDesktop.loadFailed" })}：${message}`);
    } finally {
      setBusy(false);
    }
  }, [intl, reloadLanState]);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      // LAN 状态与系统远程桌面状态各读各的；读失败按 fail-closed（关/未知）展示。
      const [lanState, rdResult] = await Promise.allSettled([
        platform.getLanAccessState(),
        platform.rdGetServerState(),
      ]);
      if (cancelled) {
        return;
      }
      if (lanState.status === "fulfilled") {
        applyState(lanState.value);
      } else {
        setError(
          lanState.reason instanceof Error ? lanState.reason.message : String(lanState.reason),
        );
      }
      setRdState(rdResult.status === "fulfilled" ? rdResult.value : EMPTY_RD_STATE);
      setLoading(false);
    })();
    return () => {
      cancelled = true;
    };
  }, [platform]);

  // 配对码倒计时：只在有配对码时按秒刷新。
  useEffect(() => {
    if (!state.pairCode) {
      return;
    }
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [state.pairCode]);

  const run = useCallback(
    async (action: () => Promise<LanAccessState>, failureMessage: string) => {
      setBusy(true);
      try {
        applyState(await action());
      } catch (caught) {
        const message = caught instanceof Error ? caught.message : String(caught);
        setError(message);
        toast(`${failureMessage}：${message}`);
      } finally {
        setBusy(false);
      }
    },
    [],
  );

  /**
   * 提示性共享开关：写 main 凭据存储 `remote-desktop:server-enabled`。
   * 关掉它**不会**断开任何 RDP 连接（系统 RDP 独立监听 3389，本应用拦不住）。
   */
  const toggleSharing = useCallback(
    async (enabled: boolean) => {
      setBusy(true);
      try {
        setRdState(await platform.rdSetServerEnabled(enabled));
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

  const systemRdpFacet = resolveSystemRdpFacet(rdState);
  const guidanceVisible = systemRdpFacet !== "enabled" && systemRdpFacet !== "unknown";
  const guidanceText = intl.formatMessage({ id: "settings.remoteDesktop.systemRdp.guidance" });

  const copyGuidance = useCallback(() => {
    void navigator.clipboard
      .writeText(guidanceText)
      .then(() => toast(intl.formatMessage({ id: "settings.remoteDesktop.systemRdp.guidanceCopied" })))
      .catch((caught: unknown) => {
        logger.warn(
          "[remote-desktop] 复制开启指引失败:",
          caught instanceof Error ? caught.message : String(caught),
        );
      });
  }, [guidanceText, intl]);

  const statusText = state.enabled
    ? intl.formatMessage(
        { id: "settings.lanAccess.statusEnabled" },
        {
          // 本机信息的展示归宿：全部监听地址（含 Tailscale）一次给全——
          // 发现列表已过滤本机，这里就是看本机地址的地方。
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
                onCheckedChange={(checked) =>
                  void run(
                    () => platform.setLanAccessEnabled(checked),
                    intl.formatMessage({ id: "settings.lanAccess.toggleFailed" }),
                  )
                }
              />
            )
          }
        />
        {/* 提示性共享开关：明写不影响系统远程桌面（spec 产品规则 5）。 */}
        <SettingsRow
          label={intl.formatMessage({ id: "settings.remoteDesktop.sharing.label" })}
          description={
            rdState.sharingEnabled
              ? intl.formatMessage({ id: "settings.remoteDesktop.sharing.on" })
              : intl.formatMessage({ id: "settings.remoteDesktop.sharing.off" })
          }
          control={
            loading ? (
              <LoaderIcon className="size-4 animate-spin text-foreground-subtle" />
            ) : (
              <Switch
                checked={rdState.sharingEnabled}
                disabled={busy}
                aria-label={intl.formatMessage({ id: "settings.remoteDesktop.sharing.label" })}
                onCheckedChange={(checked) => void toggleSharing(checked)}
              />
            )
          }
          detail={
            <p
              data-testid="remote-desktop-sharing-hint"
              className="text-ui-xs text-foreground-subtle"
            >
              {intl.formatMessage({ id: "settings.remoteDesktop.sharing.hint" })}
            </p>
          }
        />
        {/* 系统远程桌面状态行：只读探测结果 + 未启用时的可复制指引。 */}
        <SettingsRow
          label={intl.formatMessage({ id: "settings.remoteDesktop.systemRdp.label" })}
          description={intl.formatMessage({
            id: `settings.remoteDesktop.systemRdp.status.${systemRdpFacet}`,
          })}
          control={
            loading ? (
              <LoaderIcon className="size-4 animate-spin text-foreground-subtle" />
            ) : (
              <span
                data-testid="remote-desktop-system-rdp-status"
                data-facet={systemRdpFacet}
                className={cn(
                  "text-ui-base",
                  systemRdpFacet === "enabled" ? "text-success" : "text-foreground-subtle",
                )}
              >
                {intl.formatMessage({
                  id: `settings.remoteDesktop.systemRdp.badge.${systemRdpFacet}`,
                })}
              </span>
            )
          }
          detail={
            guidanceVisible ? (
              <div className="space-y-2 rounded-lg border border-dashed border-border bg-surface px-3 py-2">
                <pre className="whitespace-pre-wrap font-sans text-ui-xs text-foreground-subtle">
                  {guidanceText}
                </pre>
                <Button type="button" variant="outline" size="sm" onClick={copyGuidance}>
                  <Copy className="size-3.5" />
                  {intl.formatMessage({ id: "settings.remoteDesktop.systemRdp.copyGuidance" })}
                </Button>
              </div>
            ) : undefined
          }
        />
        {state.enabled ? (
          <SettingsRow
            label={intl.formatMessage({ id: "settings.lanAccess.pairCode" })}
            description={
              state.pairCode
                ? intl.formatMessage(
                    { id: "settings.lanAccess.pairCodeExpires" },
                    { time: formatRemaining(state.pairCode.expiresAt, now) },
                  )
                : intl.formatMessage({ id: "settings.lanAccess.pairCodeHint" })
            }
            control={
              state.pairCode ? (
                <div className="flex items-center gap-2">
                  <span className="font-mono text-ui-lg tracking-[0.3em] text-foreground">
                    {state.pairCode.code}
                  </span>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    disabled={busy}
                    onClick={() =>
                      void run(
                        () => platform.createLanAccessPairCode(),
                        intl.formatMessage({ id: "settings.lanAccess.pairCodeFailed" }),
                      )
                    }
                  >
                    {intl.formatMessage({ id: "settings.lanAccess.refreshPairCode" })}
                  </Button>
                </div>
              ) : (
                <Button
                  type="button"
                  variant="outline"
                  size="lg"
                  disabled={busy}
                  onClick={() =>
                    void run(
                      () => platform.createLanAccessPairCode(),
                      intl.formatMessage({ id: "settings.lanAccess.pairCodeFailed" }),
                    )
                  }
                >
                  {intl.formatMessage({ id: "settings.lanAccess.createPairCode" })}
                </Button>
              )
            }
          />
        ) : null}
        <SettingsRow
          label={intl.formatMessage({ id: "settings.lanAccess.clients" })}
          description={
            state.clients.length === 0
              ? intl.formatMessage({ id: "settings.lanAccess.noClients" })
              : intl.formatMessage(
                  { id: "settings.lanAccess.clientCount" },
                  { count: String(state.clients.length) },
                )
          }
          control={
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={busy || loading}
              onClick={() => void refresh()}
            >
              <RefreshCw className="size-4" />
              {intl.formatMessage({ id: "settings.lanAccess.refreshPairedPeers" })}
            </Button>
          }
          detail={
            state.clients.length > 0 ? (
              <RemoteControlPairedClientsList
                clients={state.clients}
                busy={busy}
                onRemove={(clientId) =>
                  void run(
                    () => platform.removeLanAccessClient(clientId),
                    intl.formatMessage({ id: "settings.lanAccess.removeFailed" }),
                  )
                }
                onResetTokens={() =>
                  void run(
                    () => platform.resetLanAccessTokens(),
                    intl.formatMessage({ id: "settings.lanAccess.resetFailed" }),
                  )
                }
              />
            ) : undefined
          }
        />
      </SettingsGroupCard>

      {/* 「我配对的对端」（客户端视角）：与上面的被控分组分列展示；本机开关关闭时也显示。 */}
      <LanAccessPairedPeersCard onError={setError} />

      <div className="space-y-2 px-1 text-ui-base text-foreground-subtle">
        <p className="flex items-start gap-1.5">
          {state.enabled ? (
            <CheckIcon className="mt-0.5 size-3.5 shrink-0 text-success" />
          ) : (
            <Radar className="mt-0.5 size-3.5 shrink-0" />
          )}
          {intl.formatMessage({
            id: state.enabled ? "settings.lanAccess.firewallHint" : "settings.lanAccess.description",
          })}
        </p>
        {/* 提示：本应用拦不住 RDP 连接（spec 已知限制）。 */}
        <p className="flex items-start gap-1.5">
          <ShieldCheck className="mt-0.5 size-3.5 shrink-0" />
          {intl.formatMessage({ id: "settings.remoteDesktop.securityHint" })}
        </p>
        <p className="flex items-start gap-1.5">
          <MonitorSmartphone className="mt-0.5 size-3.5 flex-none" />
          {intl.formatMessage({ id: "settings.remoteDesktop.lanHint" })}
        </p>
        {rdState.systemRdp?.editionOk === false ? (
          <p className="flex items-start gap-1.5 text-warning">
            <AlertTriangleIcon className="mt-0.5 size-3.5 shrink-0" />
            {intl.formatMessage({ id: "settings.remoteDesktop.systemRdp.homeEditionNote" })}
          </p>
        ) : null}
      </div>
      {error ? <p className="px-1 text-ui-base text-destructive">{error}</p> : null}
    </div>
  );
}
