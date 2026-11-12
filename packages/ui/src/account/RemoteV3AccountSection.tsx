import { useCallback, useEffect, useRef, useState } from "react";
import { ExternalLink, Fingerprint, LoaderIcon, LogOut, Server, ShieldCheck } from "lucide-react";
import type { RemoteV3GithubChallenge, RemoteV3HostState } from "@mode/shared";

import { Button } from "@/ui-foundation/adapters/button.js";
import { Input } from "@/ui-foundation/adapters/input.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { useServices } from "@/hooks/useServices.js";
import { useOptionalBaseWorkspaceServices } from "@/hooks/useWorkspaceServices.js";
import { useOptionalPlatform } from "@/hooks/usePlatform.js";

const STATE_TIMEOUT_MS = 12_000;

/**
 * 远程控制 v3 账号登录面（docs/specs/remote-control-v3.md）。
 *
 * 与 v2 Mode Cloud 分区不同：登录后本机是带 X25519 设备身份的 Host；设备目录只返回
 * descriptor/在线状态/地址，不返回任何业务令牌。UI 只读快照并下发登录/退出命令。
 */
export function RemoteV3AccountSection({ active }: { active: boolean }) {
  const services = useServices();
  const baseServices = useOptionalBaseWorkspaceServices();
  const service = baseServices?.remoteV3Service ?? services.remoteV3Service;
  const platform = useOptionalPlatform();
  const { intl } = useModeIntl();
  const [state, setState] = useState<RemoteV3HostState | null>(null);
  const [challenge, setChallenge] = useState<RemoteV3GithubChallenge | null>(null);
  const [serverUrl, setServerUrl] = useState("");
  const [busy, setBusy] = useState<"login" | "disconnect" | null>(null);
  const [error, setError] = useState("");
  const [available, setAvailable] = useState(true);

  const message = (id: string, values?: Record<string, string>): string =>
    intl.formatMessage({ id }, values);

  const load = useCallback(async () => {
    if (!service) {
      setAvailable(false);
      return;
    }
    try {
      const next = await withTimeout(service.getState(), STATE_TIMEOUT_MS);
      setState(next);
      setAvailable(true);
      setError("");
      setServerUrl((current) => current || next.serverUrl || "");
    } catch {
      // 旧 Host / Web 没有 remote-v3 频道：隐藏整个账号登录面。
      setAvailable(false);
    }
  }, [service]);

  useEffect(() => {
    if (active) void load();
  }, [active, load]);

  useEffect(() => {
    if (!service) return;
    const subscription = service.onDidChange((next) => {
      setState(next);
      if (next.status === "online") {
        setChallenge(null);
        // 自动重连成功后清掉上一轮错误提示，避免「已恢复但仍显示连接中断」。
        setError("");
      } else if (next.error) {
        setError(next.error);
      }
    });
    return () => subscription.dispose();
  }, [service]);

  const openedCodes = useRef(new Set<string>());
  const openAuthorization = useCallback(
    (pendingLogin: { userCode: string; verificationUri: string }): void => {
      if (openedCodes.current.has(pendingLogin.userCode)) return;
      openedCodes.current.add(pendingLogin.userCode);
      // 直接唤起系统浏览器；验证码同时放进剪贴板，用户只需粘贴 + 授权。
      try {
        if (platform) platform.openExternal(pendingLogin.verificationUri);
        else window.open(pendingLogin.verificationUri, "_blank", "noopener,noreferrer");
      } catch {
        // 打开失败时保留页面上的链接兜底。
      }
      void navigator.clipboard?.writeText(pendingLogin.userCode).catch(() => undefined);
    },
    [platform],
  );

  useEffect(() => {
    if (!active) return;
    const pendingLogin = challenge ?? state?.pendingLogin;
    if (!pendingLogin) return;
    openAuthorization(pendingLogin);
  }, [active, challenge, state?.pendingLogin, openAuthorization]);

  if (!service || !available) return null;

  const status = state?.status ?? "unconfigured";
  const pending =
    challenge ??
    (state?.pendingLogin
      ? {
          userCode: state.pendingLogin.userCode,
          verificationUri: state.pendingLogin.verificationUri,
          expiresIn: Math.max(0, Math.floor((state.pendingLogin.expiresAt - Date.now()) / 1000)),
          interval: 5,
        }
      : null);

  const beginLogin = async (): Promise<void> => {
    setBusy("login");
    setError("");
    setChallenge(null);
    try {
      const next = await service.beginGithubLogin({
        serverUrl: serverUrl.trim() || undefined,
      });
      setChallenge(next);
      openAuthorization(next);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setBusy(null);
    }
  };

  const cancelLogin = (): void => {
    service.cancelGithubLogin();
    setChallenge(null);
  };

  const disconnect = async (): Promise<void> => {
    setBusy("disconnect");
    setError("");
    try {
      setState(await service.disconnectAccount());
      setChallenge(null);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setBusy(null);
    }
  };

  const loginPanel = (
    <div className="mt-3 space-y-3" data-testid="mode-account-remote-v3-login">
      <div>
        <label
          className="text-ui-xs text-foreground-subtle"
          htmlFor="mode-account-remote-v3-server"
        >
          {message("modeAccount.remoteV3.serverUrl")}
        </label>
        <Input
          id="mode-account-remote-v3-server"
          className="mt-1"
          value={serverUrl}
          placeholder={message("modeAccount.remoteV3.serverUrlPlaceholder")}
          autoComplete="off"
          spellCheck={false}
          disabled={busy !== null || pending !== null}
          onChange={(event) => setServerUrl(event.target.value)}
        />
      </div>
      {pending ? (
        <div className="rounded-lg border border-border bg-background px-3 py-3">
          <div className="flex items-center gap-2 text-ui-base text-foreground">
            <Fingerprint className="size-4 text-foreground-subtle" aria-hidden="true" />
            {message("modeAccount.remoteV3.githubCodeTitle")}
          </div>
          <div
            className="mt-2 text-2xl font-semibold tracking-[0.3em] text-foreground"
            data-testid="mode-account-remote-v3-github-code"
          >
            {pending.userCode}
          </div>
          {pending.verificationUri.startsWith("https://github.com/") ? (
            <a
              className="mt-2 inline-flex items-center gap-1 text-ui-base text-sky-600 hover:underline dark:text-sky-400"
              href={pending.verificationUri}
              target="_blank"
              rel="noreferrer"
            >
              <ExternalLink className="size-3.5" aria-hidden="true" />
              {message("modeAccount.remoteV3.githubOpenUri")}
            </a>
          ) : null}
          <p className="mt-2 flex items-center gap-2 text-ui-xs text-foreground-subtlest">
            <LoaderIcon className="size-3 animate-spin" />
            {message("modeAccount.remoteV3.githubWaiting")}
          </p>
          <Button type="button" variant="ghost" size="sm" className="mt-2" onClick={cancelLogin}>
            {message("modeAccount.remoteV3.cancel")}
          </Button>
        </div>
      ) : (
        <Button
          type="button"
          variant="outline"
          size="sm"
          data-testid="mode-account-remote-v3-github-login"
          disabled={busy !== null}
          onClick={() => void beginLogin()}
        >
          {busy === "login" ? (
            <LoaderIcon className="size-4 animate-spin" />
          ) : (
            <Fingerprint className="size-4" />
          )}
          {message("modeAccount.remoteV3.login")}
        </Button>
      )}
      {error ? (
        <p className="text-ui-base text-destructive" data-testid="mode-account-remote-v3-error">
          {error}
        </p>
      ) : null}
    </div>
  );

  return (
    <section
      className="rounded-xl border border-border bg-card px-4 py-3"
      data-testid="mode-account-remote-v3"
    >
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <ShieldCheck className="size-4 text-foreground-subtle" aria-hidden="true" />
          <h2 className="text-ui-lg font-medium text-foreground">
            {message("modeAccount.remoteV3.title")}
          </h2>
          <span
            className={`size-1.5 rounded-full ${
              status === "online"
                ? "bg-emerald-500"
                : status === "error"
                  ? "bg-destructive"
                  : status === "connecting" || status === "reconnecting"
                    ? "bg-amber-500"
                    : "bg-foreground-subtlest"
            }`}
            aria-hidden="true"
          />
          <span className="text-ui-xs text-foreground-subtle">
            {message(`modeAccount.remoteV3.status.${status}`)}
          </span>
        </div>
      </div>
      <p className="mt-1 text-ui-base text-foreground-subtle">
        {message("modeAccount.remoteV3.description")}
      </p>

      {status === "connecting" || status === "reconnecting" ? (
        <p className="mt-3 flex items-center gap-2 text-ui-base text-foreground-subtle">
          <LoaderIcon className="size-4 animate-spin" />
          {message(
            status === "reconnecting"
              ? "modeAccount.remoteV3.reconnecting"
              : "modeAccount.remoteV3.connecting",
          )}
        </p>
      ) : null}

      {status === "unconfigured" || status === "error" ? loginPanel : null}

      {status === "online" || status === "reconnecting" ? (
        <div className="mt-3 space-y-3">
          <div className="rounded-lg border border-border bg-background px-3 py-2">
            <div className="flex items-center justify-between gap-2">
              <div className="min-w-0">
                <div className="truncate text-ui-base text-foreground">
                  {state?.deviceName || message("modeAccount.remoteV3.thisDevice")}
                </div>
                {state?.accountName ? (
                  <div className="truncate text-ui-xs text-foreground-subtle">
                    {message("modeAccount.remoteV3.account")} {state.accountName}
                  </div>
                ) : null}
                <div className="mt-0.5 flex items-center gap-1 text-ui-xs text-foreground-subtle">
                  <Server className="size-3" aria-hidden="true" />
                  <span className="truncate">{state?.serverUrl}</span>
                </div>
              </div>
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={busy !== null}
                onClick={() => void disconnect()}
              >
                {busy === "disconnect" ? (
                  <LoaderIcon className="size-4 animate-spin" />
                ) : (
                  <LogOut className="size-4" />
                )}
                {message("modeAccount.remoteV3.disconnect")}
              </Button>
            </div>
            <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-ui-xs text-foreground-subtlest">
              <span title={state?.fingerprint ?? ""}>
                {message("modeAccount.remoteV3.fingerprint")}{" "}
                {state?.fingerprint ? `${state.fingerprint.slice(0, 16)}…` : "-"}
              </span>
              <span>
                {message("modeAccount.remoteV3.activeConnections", {
                  count: String(state?.activeConnections ?? 0),
                })}
              </span>
            </div>
          </div>

          {error ? <p className="text-ui-base text-destructive">{error}</p> : null}
        </div>
      ) : null}
    </section>
  );
}

function withTimeout<T>(promise: Promise<T>, timeoutMs: number): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("remote v3 state timeout")), timeoutMs);
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error: unknown) => {
        clearTimeout(timer);
        reject(error);
      },
    );
  });
}
