import { useCallback, useEffect, useMemo, useState } from "react";
import { LoaderIcon, RefreshCw } from "lucide-react";
import type { LanAccessState } from "@mode/shared";
import { Button } from "@/ui-foundation/adapters/button.js";
import { Input } from "@/ui-foundation/adapters/input.js";
import { toast } from "@/ui-foundation/adapters/toast.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import {
  buildDesktopEnrollmentUri,
  pickPreferredLanAddress,
} from "@/remote-desktop/modeAgentPairing.js";

function formatRemaining(expiresAt: number, now: number): string {
  const seconds = Math.max(0, Math.round((expiresAt - now) / 1000));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

/**
 * 远程控制页 · 本机配对码（docs/specs/remote-control-unified-pairing.md R1/R4）：
 * 同一个 LAN 一次性配对码同时服务远程工作区、远程桌面和 Mode Agent 反向登记；
 * 地址按 LAN → Tailscale → 链路本地排序，多地址可切换，配对串随之更新。
 */
export function RemoteControlPairingCodePanel({ active }: { active: boolean }) {
  const { intl } = useModeIntl();
  const platform = usePlatform();
  const [state, setState] = useState<LanAccessState | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [now, setNow] = useState(() => Date.now());
  const [selectedAddress, setSelectedAddress] = useState("");

  // 设置页也能生成同一个一次性码：页面重新可见时重读，避免展示已被轮换的旧码。
  const loadState = useCallback(async () => {
    try {
      setState(await platform.getLanAccessState());
      setError("");
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    }
  }, [platform]);

  useEffect(() => {
    if (active) {
      void loadState();
    }
  }, [active, loadState]);

  const preferredAddress = useMemo(
    () => pickPreferredLanAddress(state?.addresses ?? []),
    [state?.addresses],
  );

  useEffect(() => {
    if (!state) return;
    setSelectedAddress((current) =>
      state.addresses.includes(current) ? current : (preferredAddress ?? state.addresses[0] ?? ""),
    );
  }, [preferredAddress, state]);

  useEffect(() => {
    if (!state?.pairCode) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [state?.pairCode]);

  const generate = useCallback(async () => {
    setBusy(true);
    setError("");
    try {
      let next = await platform.getLanAccessState();
      if (!next.enabled) {
        next = await platform.setLanAccessEnabled(true);
      }
      setState(await platform.createLanAccessPairCode());
      setNow(Date.now());
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught);
      setError(message);
      toast(`${intl.formatMessage({ id: "remoteDesktop.pairing.failed" })}：${message}`);
    } finally {
      setBusy(false);
    }
  }, [intl, platform]);

  const copy = useCallback(
    async (value: string) => {
      try {
        await navigator.clipboard.writeText(value);
        toast(intl.formatMessage({ id: "remoteDesktop.pairing.copied" }));
      } catch (caught) {
        const message = caught instanceof Error ? caught.message : String(caught);
        toast(`${intl.formatMessage({ id: "remoteDesktop.pairing.copyFailed" })}：${message}`);
      }
    },
    [intl],
  );

  const pair = state?.pairCode ?? null;
  const uri =
    pair && selectedAddress
      ? buildDesktopEnrollmentUri(selectedAddress, state?.port ?? null, pair.code)
      : "";

  return (
    <section
      className="rounded-xl border border-border bg-card px-4 py-3"
      data-testid="remote-control-pairing"
    >
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-ui-lg font-medium text-foreground">
          {intl.formatMessage({ id: "remoteDesktop.pairing.title" })}
        </h2>
        <Button
          type="button"
          variant="outline"
          size="sm"
          data-testid="remote-control-generate-code"
          disabled={busy}
          onClick={() => void generate()}
        >
          {busy ? (
            <LoaderIcon className="size-3.5 animate-spin" />
          ) : (
            <RefreshCw className="size-3.5" />
          )}
          {intl.formatMessage({
            id: pair ? "remoteDesktop.pairing.regenerate" : "remoteDesktop.pairing.generate",
          })}
        </Button>
      </div>

      {error ? <p className="mt-2 text-ui-base text-destructive">{error}</p> : null}

      {pair ? (
        <div className="mt-3 space-y-3 border-t border-border pt-3">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-ui-base text-foreground-subtle">
              {intl.formatMessage({ id: "remoteDesktop.pairing.codeLabel" })}
            </span>
            <span
              className="font-mono text-2xl tracking-[0.3em] text-foreground"
              data-testid="remote-control-pair-code"
            >
              {pair.code}
            </span>
            <span className="text-ui-xs text-foreground-subtlest">
              {intl.formatMessage(
                { id: "remoteDesktop.pairing.expires" },
                { time: formatRemaining(pair.expiresAt, now) },
              )}
            </span>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              data-testid="remote-control-copy-code"
              onClick={() => void copy(pair.code)}
            >
              {intl.formatMessage({ id: "remoteDesktop.pairing.copyCode" })}
            </Button>
          </div>

          {state && state.addresses.length > 1 ? (
            <label className="flex flex-wrap items-center gap-2 text-ui-xs text-foreground-subtle">
              <span>{intl.formatMessage({ id: "remoteDesktop.pairing.address" })}</span>
              <select
                data-testid="remote-control-pair-address"
                className="h-8 min-w-48 rounded-lg border border-input-border bg-input px-2 font-mono text-ui-xs text-foreground"
                value={selectedAddress}
                onChange={(event) => setSelectedAddress(event.target.value)}
              >
                {state.addresses.map((address) => (
                  <option key={address} value={address}>
                    {address}
                  </option>
                ))}
              </select>
            </label>
          ) : null}

          <Input
            readOnly
            size="lg"
            data-testid="remote-control-pair-uri"
            value={uri}
            onFocus={(event) => event.currentTarget.select()}
          />

          <div className="flex flex-wrap items-center gap-2">
            <Button
              type="button"
              variant="outline"
              size="sm"
              data-testid="remote-control-copy-uri"
              disabled={!uri}
              onClick={() => void copy(uri)}
            >
              {intl.formatMessage({ id: "remoteDesktop.pairing.copyUri" })}
            </Button>
          </div>

          <p className="text-ui-xs text-foreground-subtle">
            {intl.formatMessage({ id: "remoteDesktop.pairing.hint" })}
          </p>
        </div>
      ) : null}
    </section>
  );
}
