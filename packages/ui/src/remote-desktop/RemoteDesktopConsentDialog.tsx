/**
 * 被控远程桌面同意窗（LR-3，renderer 模态）。
 *
 * Tauri 产品壳没有 Electron main 的原生同意窗；Host 把同意请求经 `remote-desktop` 频道
 * 发到 renderer，本组件负责 60s 倒计时、允许/拒绝与「记住此设备」。
 * Electron 下 `onRemoteDesktopConsentRequest` 不存在（main 原生窗），组件不渲染。
 *
 * 已知边界（DR-LR-1）：窗口最小化/失焦时 renderer 模态不可见；壳 `WebviewWindow` 是后续
 * 产品决策，本组件是当前可用的 fallback。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import type { RemoteDesktopConsentRequest } from "@mode/shared/remote-desktop";

import { Button } from "@/ui-foundation/adapters/button.js";
import { Checkbox } from "@/ui-foundation/adapters/checkbox.js";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/ui-foundation/adapters/dialog.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { logger } from "@/logger.js";

const CONSENT_TIMEOUT_SECONDS = 60;

export function RemoteDesktopConsentDialog() {
  const platform = usePlatform();
  const { intl } = useModeIntl();
  const [request, setRequest] = useState<RemoteDesktopConsentRequest | null>(null);
  const [remember, setRemember] = useState(false);
  const [secondsLeft, setSecondsLeft] = useState(CONSENT_TIMEOUT_SECONDS);
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);

  useEffect(() => {
    const subscribe = platform.onRemoteDesktopConsentRequest;
    if (!subscribe) {
      return;
    }
    return subscribe((next) => {
      busyRef.current = false;
      setBusy(false);
      setRemember(false);
      setSecondsLeft(CONSENT_TIMEOUT_SECONDS);
      setRequest(next);
    });
  }, [platform]);

  const respond = useCallback(
    async (decision: "allow" | "deny", rememberDevice: boolean, reason?: "user" | "timeout") => {
      const current = request;
      if (!current || busyRef.current) {
        return;
      }
      busyRef.current = true;
      setBusy(true);
      try {
        await platform.respondRemoteDesktopConsent?.({
          clientId: current.clientId,
          decision,
          remember: rememberDevice,
          ...(reason ? { reason } : {}),
        });
      } catch (error) {
        logger.warn("[remote-desktop] 同意结果回传失败", error);
      } finally {
        busyRef.current = false;
        setBusy(false);
        setRequest(null);
      }
    },
    [platform, request],
  );

  useEffect(() => {
    if (!request) {
      return;
    }
    const timer = window.setInterval(() => {
      setSecondsLeft((current) => Math.max(0, current - 1));
    }, 1_000);
    return () => window.clearInterval(timer);
  }, [request]);

  useEffect(() => {
    if (!request || secondsLeft > 0) {
      return;
    }
    void respond("deny", false, "timeout");
  }, [request, respond, secondsLeft]);

  if (!request) {
    return null;
  }

  return (
    <Dialog open onOpenChange={() => undefined}>
      <DialogContent
        showCloseButton={false}
        className="max-w-md"
        onEscapeKeyDown={(event) => event.preventDefault()}
        onInteractOutside={({ preventDefault }) => preventDefault()}
      >
        <DialogHeader>
          <DialogTitle>
            {intl.formatMessage(
              { id: "remoteDesktop.consent.title" },
              { device: request.label || request.clientId },
            )}
          </DialogTitle>
        </DialogHeader>
        <p className="text-ui-sm text-foreground-muted">
          {intl.formatMessage({ id: "remoteDesktop.consent.timeout" }, { seconds: secondsLeft })}
        </p>
        <label className="flex items-center gap-2 text-ui-sm text-foreground">
          <Checkbox
            checked={remember}
            onCheckedChange={(checked) => setRemember(checked === true)}
            disabled={busy}
          />
          {intl.formatMessage({ id: "remoteDesktop.consent.remember" })}
        </label>
        <DialogFooter>
          <Button
            variant="outline"
            disabled={busy}
            onClick={() => void respond("deny", false, "user")}
          >
            {intl.formatMessage({ id: "remoteDesktop.consent.deny" })}
          </Button>
          <Button disabled={busy} onClick={() => void respond("allow", remember)}>
            {intl.formatMessage({ id: "remoteDesktop.consent.allow" })}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
