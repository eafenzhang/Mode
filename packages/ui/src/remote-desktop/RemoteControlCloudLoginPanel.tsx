import { useEffect, useRef, useState } from "react";
import { LoaderIcon, MessageCircle } from "lucide-react";
import type { CloudAccountState, CloudLoginChallenge } from "@mode/shared";
import type { ICloudAccountService } from "@mode/services";
import { Button } from "@/ui-foundation/adapters/button.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";

const POLL_INTERVAL_MS = 2_000;

/**
 * 微信登录面板：开始登录 → 显示 6 位码 → 轮询云端直到 bound。
 * 用户把码发给云端 Bot；Bot 只标记 approved，令牌交换仍由 Desktop 轮询完成。
 */
export function RemoteControlCloudLoginPanel({
  service,
  onBound,
  onClose,
}: {
  service: ICloudAccountService;
  onBound: (state: CloudAccountState) => void;
  onClose: () => void;
}) {
  const { intl } = useModeIntl();
  const [challenge, setChallenge] = useState<CloudLoginChallenge | null>(null);
  const [starting, setStarting] = useState(true);
  const [error, setError] = useState("");
  const activeRef = useRef(true);
  const stoppedRef = useRef(false);

  const message = (id: string): string => intl.formatMessage({ id });

  useEffect(() => {
    activeRef.current = true;
    stoppedRef.current = false;
    void (async () => {
      try {
        const next = await service.beginWechatLogin();
        if (activeRef.current) {
          setChallenge(next);
        }
      } catch (caught) {
        if (activeRef.current) {
          setError(caught instanceof Error ? caught.message : String(caught));
        }
      } finally {
        if (activeRef.current) {
          setStarting(false);
        }
      }
    })();
    return () => {
      activeRef.current = false;
      service.cancelWechatLogin();
    };
  }, [service]);

  useEffect(() => {
    if (!challenge) {
      return;
    }
    let cancelled = false;
    const poll = async (): Promise<void> => {
      if (cancelled || stoppedRef.current) {
        return;
      }
      try {
        const next = await service.pollWechatLogin();
        if (cancelled) {
          return;
        }
        if (next.status === "bound") {
          onBound(next);
        }
      } catch (caught) {
        if (!cancelled) {
          stoppedRef.current = true;
          setError(caught instanceof Error ? caught.message : String(caught));
        }
      }
    };
    const timer = setInterval(() => void poll(), POLL_INTERVAL_MS);
    void poll();
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [challenge, onBound, service]);

  return (
    <div className="mt-2 rounded-lg border border-border bg-background px-3 py-3">
      <div className="flex items-center gap-2 text-ui-base text-foreground">
        <MessageCircle className="size-4 text-foreground-subtle" aria-hidden="true" />
        {message("remoteDesktop.cloud.wechatLogin")}
      </div>
      {starting ? (
        <p className="mt-2 flex items-center gap-2 text-ui-base text-foreground-subtle">
          <LoaderIcon className="size-4 animate-spin" />
          {message("remoteDesktop.cloud.wechatStarting")}
        </p>
      ) : challenge ? (
        <>
          <p className="mt-2 text-ui-base text-foreground-subtle">
            {message("remoteDesktop.cloud.wechatCodeTitle")}
          </p>
          <div
            className="mt-2 text-2xl font-semibold tracking-[0.3em] text-foreground"
            data-testid="remote-control-cloud-login-code"
          >
            {challenge.code}
          </div>
          <p className="mt-2 flex items-center gap-2 text-ui-xs text-foreground-subtlest">
            <LoaderIcon className="size-3 animate-spin" />
            {message("remoteDesktop.cloud.wechatWaiting")}
          </p>
        </>
      ) : null}
      {error ? (
        <p className="mt-2 text-ui-base text-destructive">
          {message("remoteDesktop.cloud.wechatLoginFailed")}：{error}
        </p>
      ) : null}
      <Button
        type="button"
        variant="ghost"
        size="sm"
        className="mt-2"
        onClick={onClose}
      >
        {message("remoteDesktop.cloud.wechatCancel")}
      </Button>
    </div>
  );
}
