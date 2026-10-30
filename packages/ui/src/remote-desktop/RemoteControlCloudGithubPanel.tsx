import { useEffect, useRef, useState } from "react";
import { ExternalLink, Fingerprint, LoaderIcon } from "lucide-react";
import type { CloudAccountState, CloudGithubLoginChallenge } from "@mode/shared";
import type { ICloudAccountService } from "@mode/services";
import { Button } from "@/ui-foundation/adapters/button.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";

/** 与 GitHub Device Flow 默认 interval 对齐；云端不参与 GitHub 轮询节奏。 */
const POLL_INTERVAL_MS = 5_000;

/**
 * GitHub Device Flow 登录面板：显示 user code + 授权链接，轮询直到 bound。
 * verificationUri 只在 https://github.com/ 域名下渲染为链接，避免云端返回值变成任意跳转。
 */
export function RemoteControlCloudGithubPanel({
  service,
  onBound,
  onClose,
}: {
  service: ICloudAccountService;
  onBound: (state: CloudAccountState) => void;
  onClose: () => void;
}) {
  const { intl } = useModeIntl();
  const [challenge, setChallenge] = useState<CloudGithubLoginChallenge | null>(null);
  const [starting, setStarting] = useState(true);
  const [error, setError] = useState("");
  const activeRef = useRef(true);
  const stoppedRef = useRef(false);

  const message = (id: string): string => intl.formatMessage({ id });
  const verificationUrl = challenge?.verificationUri ?? "";
  const safeVerification =
    verificationUrl.startsWith("https://github.com/") ? verificationUrl : "";

  useEffect(() => {
    activeRef.current = true;
    stoppedRef.current = false;
    void (async () => {
      try {
        const next = await service.beginGithubLogin();
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
      service.cancelGithubLogin();
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
        const next = await service.pollGithubLogin();
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
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [challenge, onBound, service]);

  return (
    <div className="mt-2 rounded-lg border border-border bg-background px-3 py-3">
      <div className="flex items-center gap-2 text-ui-base text-foreground">
        <Fingerprint className="size-4 text-foreground-subtle" aria-hidden="true" />
        {message("remoteDesktop.cloud.githubLogin")}
      </div>
      {starting ? (
        <p className="mt-2 flex items-center gap-2 text-ui-base text-foreground-subtle">
          <LoaderIcon className="size-4 animate-spin" />
          {message("remoteDesktop.cloud.githubStarting")}
        </p>
      ) : challenge ? (
        <>
          <p className="mt-2 text-ui-base text-foreground-subtle">
            {message("remoteDesktop.cloud.githubCodeTitle")}
          </p>
          <div
            className="mt-2 text-2xl font-semibold tracking-[0.3em] text-foreground"
            data-testid="remote-control-cloud-github-code"
          >
            {challenge.userCode}
          </div>
          {safeVerification ? (
            <a
              className="mt-2 inline-flex items-center gap-1 text-ui-base text-sky-600 hover:underline dark:text-sky-400"
              href={safeVerification}
              target="_blank"
              rel="noreferrer"
              data-testid="remote-control-cloud-github-verify-link"
            >
              <ExternalLink className="size-3.5" aria-hidden="true" />
              {message("remoteDesktop.cloud.githubOpenUri")}
            </a>
          ) : null}
          <p className="mt-2 flex items-center gap-2 text-ui-xs text-foreground-subtlest">
            <LoaderIcon className="size-3 animate-spin" />
            {message("remoteDesktop.cloud.githubWaiting")}
          </p>
        </>
      ) : null}
      {error ? (
        <p className="mt-2 text-ui-base text-destructive">
          {message("remoteDesktop.cloud.githubLoginFailed")}：{error}
        </p>
      ) : null}
      <Button type="button" variant="ghost" size="sm" className="mt-2" onClick={onClose}>
        {message("remoteDesktop.cloud.githubCancel")}
      </Button>
    </div>
  );
}
