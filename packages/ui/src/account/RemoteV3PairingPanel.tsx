import { useEffect, useReducer, useState } from "react";
import { CheckCircle2, Copy, KeyRound, Link2, LoaderIcon } from "lucide-react";
import type { IRemoteV3Service } from "@mode/services";

import { Button } from "@/ui-foundation/adapters/button.js";
import { Input } from "@/ui-foundation/adapters/input.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import {
  INITIAL_REMOTE_V3_PAIRING_UI,
  reduceRemoteV3Pairing,
} from "./pairingModel.js";

const POLL_INTERVAL_MS = 2_000;

/**
 * 跨账号配对面板（docs/specs/remote-control-v3-pairing.md）。
 *
 * 能力按方法是否存在分角色渲染：桌面 Host 有 begin/confirm，浏览器有 claim/redeem；
 * 两者都只有 Host 本机确认才能推进，远程 Client 无法代提交验证码。
 */
export function RemoteV3PairingPanel({ service }: { service: IRemoteV3Service }) {
  const { intl } = useModeIntl();
  const [state, dispatch] = useReducer(reduceRemoteV3Pairing, INITIAL_REMOTE_V3_PAIRING_UI);
  const [ticketInput, setTicketInput] = useState("");
  const [busy, setBusy] = useState<"host" | "confirm" | "claim" | null>(null);
  const [copied, setCopied] = useState(false);

  const canHost = Boolean(service.beginPairing && service.confirmPairing);
  const canClient = Boolean(service.claimPairing && service.redeemPairing);

  const message = (id: string): string => intl.formatMessage({ id });

  // Host：等待对方 claim（被认领后停止轮询，改为等本机输入验证码）。
  useEffect(() => {
    if (state.kind !== "host" || state.claimed || !service.getPairing) return;
    let cancelled = false;
    const timer = setInterval(() => {
      void (async () => {
        try {
          const next = await service.getPairing!(state.pairingId);
          if (cancelled) return;
          if (next.status === "claimed") dispatch({ type: "host-claimed" });
          else if (next.status === "confirmed") dispatch({ type: "host-done" });
          else if (next.status === "canceled" || next.status === "expired") {
            dispatch({ type: "failed", message: message("modeAccount.remoteV3.pairing.expired") });
          }
        } catch {
          // 瞬时轮询失败：下一拍重试，不打断配对会话。
        }
      })();
    }, POLL_INTERVAL_MS);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [state, service, intl]);

  // Client：等 Host 本机确认后一次性 redeem。
  useEffect(() => {
    if (state.kind !== "client" || !service.getPairing || !service.redeemPairing) return;
    let cancelled = false;
    let redeeming = false;
    const timer = setInterval(() => {
      void (async () => {
        if (redeeming) return;
        try {
          const next = await service.getPairing!(state.pairingId);
          if (cancelled) return;
          if (next.status === "confirmed") {
            redeeming = true;
            await service.redeemPairing!(state.pairingId);
            if (!cancelled) dispatch({ type: "client-done" });
            return;
          }
          if (next.status === "canceled" || next.status === "expired") {
            dispatch({ type: "failed", message: message("modeAccount.remoteV3.pairing.expired") });
          }
        } catch {
          // 下一拍重试。
        }
      })();
    }, POLL_INTERVAL_MS);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [state, service, intl]);

  if (!canHost && !canClient) return null;

  const errorText = (error: unknown): string =>
    error instanceof Error ? error.message : String(error);

  const startHost = async (): Promise<void> => {
    if (!service.beginPairing) return;
    setBusy("host");
    try {
      const started = await service.beginPairing();
      dispatch({
        type: "host-started",
        pairingId: started.pairingId,
        ticket: started.ticket,
        expiresAt: started.expiresAt,
      });
    } catch (error) {
      dispatch({ type: "failed", message: errorText(error) });
    } finally {
      setBusy(null);
    }
  };

  const confirmHost = async (): Promise<void> => {
    if (state.kind !== "host" || !service.confirmPairing) return;
    setBusy("confirm");
    try {
      await service.confirmPairing({ pairingId: state.pairingId, code: state.code.trim() });
      dispatch({ type: "host-done" });
    } catch (error) {
      dispatch({ type: "failed", message: errorText(error) });
    } finally {
      setBusy(null);
    }
  };

  const claimClient = async (): Promise<void> => {
    if (!service.claimPairing) return;
    setBusy("claim");
    try {
      const claim = await service.claimPairing(ticketInput.trim());
      dispatch({
        type: "client-claimed",
        pairingId: claim.pairingId,
        verificationCode: claim.verificationCode,
        hostName: claim.hostName,
        expiresAt: claim.expiresAt,
      });
    } catch (error) {
      dispatch({ type: "failed", message: errorText(error) });
    } finally {
      setBusy(null);
    }
  };

  const copyTicket = async (ticket: string): Promise<void> => {
    try {
      await navigator.clipboard.writeText(ticket);
      setCopied(true);
      setTimeout(() => setCopied(false), 1_500);
    } catch {
      // 剪贴板不可用：票仍然可手动选中复制。
    }
  };

  const reset = (): void => {
    setTicketInput("");
    dispatch({ type: "reset" });
  };

  return (
    <div className="space-y-2 rounded-lg border border-border bg-background px-3 py-3">
      <h3 className="flex items-center gap-2 text-ui-base font-medium text-foreground">
        <Link2 className="size-4 text-foreground-subtle" aria-hidden="true" />
        {message("modeAccount.remoteV3.pairing.title")}
      </h3>

      {state.kind === "idle" && canHost ? (
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={busy !== null}
          onClick={() => void startHost()}
        >
          {busy === "host" ? <LoaderIcon className="size-4 animate-spin" /> : <Link2 className="size-4" />}
          {message("modeAccount.remoteV3.pairing.hostAction")}
        </Button>
      ) : null}

      {state.kind === "idle" && canClient ? (
        <div className="space-y-2">
          <label className="text-ui-xs text-foreground-subtle" htmlFor="remote-v3-pairing-ticket">
            {message("modeAccount.remoteV3.pairing.clientHint")}
          </label>
          <div className="flex gap-2">
            <Input
              id="remote-v3-pairing-ticket"
              value={ticketInput}
              autoComplete="off"
              spellCheck={false}
              onChange={(event) => setTicketInput(event.target.value)}
            />
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={busy !== null || !ticketInput.trim()}
              onClick={() => void claimClient()}
            >
              {busy === "claim" ? <LoaderIcon className="size-4 animate-spin" /> : null}
              {message("modeAccount.remoteV3.pairing.claim")}
            </Button>
          </div>
        </div>
      ) : null}

      {state.kind === "host" ? (
        <div className="space-y-2">
          <label className="text-ui-xs text-foreground-subtle" htmlFor="remote-v3-pairing-out">
            {message("modeAccount.remoteV3.pairing.ticketLabel")}
          </label>
          <div className="flex gap-2">
            <Input id="remote-v3-pairing-out" value={state.ticket} readOnly />
            <Button type="button" variant="outline" size="sm" onClick={() => void copyTicket(state.ticket)}>
              <Copy className="size-4" />
              {copied
                ? message("modeAccount.remoteV3.pairing.copied")
                : message("modeAccount.remoteV3.pairing.copy")}
            </Button>
          </div>
          {state.claimed ? (
            <div className="space-y-2">
              <label className="text-ui-xs text-foreground-subtle" htmlFor="remote-v3-pairing-code">
                {message("modeAccount.remoteV3.pairing.codeLabel")}
              </label>
              <div className="flex gap-2">
                <Input
                  id="remote-v3-pairing-code"
                  value={state.code}
                  inputMode="numeric"
                  maxLength={8}
                  autoComplete="off"
                  onChange={(event) => dispatch({ type: "host-code", code: event.target.value })}
                />
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={busy !== null || state.code.trim().length !== 8}
                  onClick={() => void confirmHost()}
                >
                  {busy === "confirm" ? <LoaderIcon className="size-4 animate-spin" /> : <KeyRound className="size-4" />}
                  {message("modeAccount.remoteV3.pairing.confirm")}
                </Button>
              </div>
            </div>
          ) : (
            <p className="flex items-center gap-2 text-ui-xs text-foreground-subtle">
              <LoaderIcon className="size-3 animate-spin" />
              {message("modeAccount.remoteV3.pairing.hostHint")}
            </p>
          )}
        </div>
      ) : null}

      {state.kind === "client" ? (
        <div className="space-y-2">
          <p className="text-ui-xs text-foreground-subtle">{state.hostName}</p>
          <div className="text-2xl font-semibold tracking-[0.3em] text-foreground">
            {state.verificationCode}
          </div>
          <p className="text-ui-xs text-foreground-subtle">
            {message("modeAccount.remoteV3.pairing.clientCodeHint")}
          </p>
          <p className="flex items-center gap-2 text-ui-xs text-foreground-subtle">
            <LoaderIcon className="size-3 animate-spin" />
            {message("modeAccount.remoteV3.pairing.waitingConfirm")}
          </p>
        </div>
      ) : null}

      {state.kind === "host-done" || state.kind === "client-done" ? (
        <div className="flex items-center justify-between gap-2">
          <p className="flex items-center gap-2 text-ui-base text-foreground">
            <CheckCircle2 className="size-4 text-emerald-500" aria-hidden="true" />
            {message("modeAccount.remoteV3.pairing.done")}
          </p>
          <Button type="button" variant="ghost" size="sm" onClick={reset}>
            {message("modeAccount.remoteV3.pairing.again")}
          </Button>
        </div>
      ) : null}

      {state.kind === "error" ? (
        <div className="space-y-2">
          <p className="text-ui-base text-destructive">{state.message}</p>
          <Button type="button" variant="ghost" size="sm" onClick={reset}>
            {message("modeAccount.remoteV3.pairing.again")}
          </Button>
        </div>
      ) : null}
    </div>
  );
}
