import { useCallback, useEffect, useState } from "react";
import {
  CheckIcon,
  LoaderIcon,
  MonitorSmartphone,
  Radar,
  RefreshCw,
  ShieldCheck,
  ShieldOff,
  Trash2,
} from "lucide-react";
import type { LanAccessClientRecord, LanAccessState } from "@mode/shared";
import { Button } from "@/components/ui/button.js";
import { Switch } from "@/components/ui/switch.js";
import { toast } from "@/components/ui/toast.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { SettingsGroupCard, SettingsRow } from "@/settings/SettingsPageParts.js";
import { LanAccessPairedPeersCard } from "@/settings/LanAccessPairedPeersCard.js";

const EMPTY_STATE: LanAccessState = {
  enabled: false,
  port: null,
  addresses: [],
  pairCode: null,
  clients: [],
};

function formatRemaining(expiresAt: number, now: number): string {
  const remainingSeconds = Math.max(0, Math.round((expiresAt - now) / 1000));
  const minutes = Math.floor(remainingSeconds / 60);
  const seconds = remainingSeconds % 60;
  return `${minutes}:${String(seconds).padStart(2, "0")}`;
}

/**
 * 设置页「远程控制」分区（2026-10-13 合并原「远程桌面」与「局域网访问」两张卡片）：
 * - 本机被控卡片：允许局域网访问 / 允许被远程控制两个总开关、配对码、已配对设备单列表；
 * - 已配对设备列表行 = LAN 配对记录本身：名称 / 最近使用 / 最近工作区 /
 *   远程桌面授权开关（写记录上的 `remoteDesktopAuthorized`，与同意窗「记住此设备」同字段）/
 *   移除；列表与配对记录同寿命——监听未开启时 host 从凭据读取展示，不依赖服务端在跑；
 * - 「我配对的对端」（客户端视角）保持独立卡片，与主控设备页共用同一份配对记录。
 * 规格：docs/specs/lan-remote-desktop.md UI 规则「被控端」、docs/specs/lan-paired-devices.md。
 */
export function RemoteControlSettingsSection() {
  const { intl } = useModeIntl();
  const platform = usePlatform();
  const [state, setState] = useState<LanAccessState>(EMPTY_STATE);
  const [rdEnabled, setRdEnabled] = useState(false);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [now, setNow] = useState(() => Date.now());

  const applyState = (next: LanAccessState) => {
    setState(next);
    setError("");
  };

  /** 只重读 LAN 状态（授权/撤销开关改的是记录上的标记，单列表行内状态要立刻跟上）。 */
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
      // 两个总开关的状态各读各的；读失败按 fail-closed（关）展示，写路径仍以 main 现读为准。
      const [lanState, rdState] = await Promise.allSettled([
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
      setRdEnabled(rdState.status === "fulfilled" ? rdState.value.enabled : false);
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

  /** 被控总开关（允许被远程控制）：关开关 main 会立即终止进行中的被控会话。 */
  const toggleRdEnabled = useCallback(
    async (enabled: boolean) => {
      setBusy(true);
      try {
        const next = await platform.rdSetServerEnabled(enabled);
        setRdEnabled(next.enabled);
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

  /** 逐设备远程桌面授权开关：与同意窗「记住此设备」写同一记录字段。 */
  const setClientAuthorized = useCallback(
    async (client: LanAccessClientRecord, authorized: boolean) => {
      setBusy(true);
      try {
        if (authorized) {
          await platform.rdAuthorizeClient(client.id);
        } else {
          await platform.rdRevokeClient(client.id);
        }
        await reloadLanState();
      } catch (caught) {
        const message = caught instanceof Error ? caught.message : String(caught);
        setError(message);
        toast(
          `${intl.formatMessage({ id: "settings.lanAccess.rdUpdateFailed" })}：${message}`,
        );
      } finally {
        setBusy(false);
      }
    },
    [intl, platform, reloadLanState],
  );

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
              <div className="space-y-2">
                <ul className="space-y-2" data-testid="remote-desktop-paired-clients">
                  {state.clients.map((client) => {
                    const name = client.label || client.id;
                    const authorized = client.remoteDesktopAuthorized === true;
                    return (
                      <li
                        key={client.id}
                        className="flex items-start justify-between gap-3 rounded-lg border border-border bg-card px-3 py-2"
                      >
                        <span className="min-w-0 flex-1">
                          <span className="block truncate text-ui-base text-foreground">
                            {name}
                          </span>
                          <span className="block text-ui-xs text-foreground-subtle">
                            {client.lastUsedAt
                              ? intl.formatMessage(
                                  { id: "settings.lanAccess.clientLastUsed" },
                                  { time: new Date(client.lastUsedAt).toLocaleString() },
                                )
                              : intl.formatMessage({ id: "settings.lanAccess.clientNeverUsed" })}
                          </span>
                          <span className="block truncate text-ui-xs text-foreground-subtle">
                            {client.lastWorkspacePath
                              ? intl.formatMessage(
                                  { id: "settings.lanAccess.lastWorkspace" },
                                  { path: client.lastWorkspacePath },
                                )
                              : intl.formatMessage({ id: "settings.lanAccess.noLastWorkspace" })}
                          </span>
                          <span className="mt-0.5 block text-ui-xs text-foreground-subtle">
                            {intl.formatMessage({
                              id: authorized
                                ? "settings.lanAccess.rdAuthorized"
                                : "settings.lanAccess.rdUnauthorized",
                            })}
                          </span>
                        </span>
                        <span className="flex shrink-0 items-center gap-1.5">
                          <Switch
                            checked={authorized}
                            disabled={busy}
                            aria-label={intl.formatMessage(
                              {
                                id: authorized
                                  ? "settings.lanAccess.rdRevokeFor"
                                  : "settings.lanAccess.rdAuthorizeFor",
                              },
                              { name },
                            )}
                            onCheckedChange={(checked) =>
                              void setClientAuthorized(client, checked)
                            }
                          />
                          <Button
                            type="button"
                            variant="ghost"
                            size="sm"
                            disabled={busy}
                            aria-label={intl.formatMessage(
                              { id: "settings.lanAccess.removeClient" },
                              { name },
                            )}
                            onClick={() =>
                              void run(
                                () => platform.removeLanAccessClient(client.id),
                                intl.formatMessage({ id: "settings.lanAccess.removeFailed" }),
                              )
                            }
                          >
                            <Trash2 className="size-4" />
                          </Button>
                        </span>
                      </li>
                    );
                  })}
                </ul>
                <div className="flex justify-end">
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    disabled={busy}
                    onClick={() =>
                      void run(
                        () => platform.resetLanAccessTokens(),
                        intl.formatMessage({ id: "settings.lanAccess.resetFailed" }),
                      )
                    }
                  >
                    <ShieldOff className="size-4" />
                    {intl.formatMessage({ id: "settings.lanAccess.resetTokens" })}
                  </Button>
                </div>
              </div>
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
        <p className="flex items-start gap-1.5">
          <ShieldCheck className="mt-0.5 size-3.5 shrink-0" />
          {intl.formatMessage({ id: "settings.remoteDesktop.securityHint" })}
        </p>
        <p className="flex items-start gap-1.5">
          <MonitorSmartphone className="mt-0.5 size-3.5 flex-none" />
          {intl.formatMessage({ id: "settings.remoteDesktop.lanHint" })}
        </p>
      </div>
      {error ? <p className="px-1 text-ui-base text-destructive">{error}</p> : null}
    </div>
  );
}
