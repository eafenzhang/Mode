import { useCallback, useEffect, useState } from "react";
import { CheckIcon, LoaderIcon, Radar, ShieldOff, Trash2 } from "lucide-react";
import type { LanAccessState } from "@mode/shared";
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
 * 局域网访问（服务端）：本机作为被连接的一方。
 * 开关 → 监听局域网；配对码 → 供对端换长期令牌；已配对设备可逐个移除或整体重置。
 */
export function LanAccessSection() {
  const { intl } = useModeIntl();
  const platform = usePlatform();
  const [state, setState] = useState<LanAccessState>(EMPTY_STATE);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [now, setNow] = useState(() => Date.now());

  const applyState = (next: LanAccessState) => {
    setState(next);
    setError("");
  };

  const run = useCallback(async (action: () => Promise<LanAccessState>, failureMessage: string) => {
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
  }, []);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const next = await platform.getLanAccessState();
        if (!cancelled) {
          applyState(next);
        }
      } catch (caught) {
        if (!cancelled) {
          setError(caught instanceof Error ? caught.message : String(caught));
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

  // 配对码倒计时：只在有配对码时按秒刷新。
  useEffect(() => {
    if (!state.pairCode) {
      return;
    }
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [state.pairCode]);

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
        {state.enabled ? (
          <>
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
                  size="lg"
                  disabled={busy || state.clients.length === 0}
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
              }
              detail={
                state.clients.length > 0 ? (
                  <ul className="space-y-2">
                    {state.clients.map((client) => (
                      <li
                        key={client.id}
                        className="flex items-center justify-between gap-3 rounded-lg border border-border bg-card px-3 py-2"
                      >
                        <span className="min-w-0">
                          <span className="block truncate text-ui-base text-foreground">
                            {client.label || client.id}
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
                        </span>
                        <Button
                          type="button"
                          variant="ghost"
                          size="sm"
                          disabled={busy}
                          aria-label={intl.formatMessage(
                            { id: "settings.lanAccess.removeClient" },
                            { name: client.label || client.id },
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
                      </li>
                    ))}
                  </ul>
                ) : undefined
              }
            />
          </>
        ) : null}
      </SettingsGroupCard>

      {/* 「我配对的对端」（客户端视角）：与上面的服务端分组分列展示；本机开关关闭时也显示。 */}
      <LanAccessPairedPeersCard onError={setError} />

      <p className="flex items-center gap-1.5 px-1 text-ui-base text-foreground-subtle">
        {state.enabled ? (
          <CheckIcon className="size-3.5 shrink-0 text-success" />
        ) : (
          <Radar className="size-3.5 shrink-0" />
        )}
        {intl.formatMessage({
          id: state.enabled ? "settings.lanAccess.firewallHint" : "settings.lanAccess.description",
        })}
      </p>
      {error ? <p className="px-1 text-ui-base text-destructive">{error}</p> : null}
    </div>
  );
}
