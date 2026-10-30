import { useCallback, useEffect, useRef, useState } from "react";
import { Cloud, ExternalLink, Fingerprint, LoaderIcon, RefreshCw } from "lucide-react";
import type { CloudAccountState, CloudDeviceSummary, CloudGithubLoginChallenge } from "@mode/shared";
import { Button } from "@/ui-foundation/adapters/button.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { useServices } from "@/hooks/useServices.js";
import { useOptionalBaseWorkspaceServices } from "@/hooks/useWorkspaceServices.js";

// 目录请求本身有 10s 预算；前端等待必须覆盖它，否则慢网会被误判为“频道不存在”而隐藏卡片。
const STATE_TIMEOUT_MS = 12_000;

/**
 * Mode Cloud 个人账号与设备目录（docs/specs/cloud-personal-accounts.md，v2）。
 *
 * 登录：仅 GitHub Device Flow——发起即由 Host 自动打开授权页并自动轮询，授权后
 * 状态自动回到 bound；目录直连（同账号即信任），设备行「连接」无 6 位配对码。
 */
export function RemoteControlCloudSection({
  active,
  onPaired,
}: {
  active: boolean;
  onPaired: () => void | Promise<void>;
}) {
  const services = useServices();
  const baseServices = useOptionalBaseWorkspaceServices();
  // Mode Cloud 账号是本机 app-global 能力；远端 workspace 的 service 集合没有该频道，
  // 直接按 context 取会让同一账号在不同页面/窗口表现不一致。
  const service = baseServices?.cloudAccountService ?? services.cloudAccountService;
  const { intl } = useModeIntl();
  const [state, setState] = useState<CloudAccountState | null>(null);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState<"refresh" | "connect" | null>(null);
  const [error, setError] = useState("");
  const [connected, setConnected] = useState<string>("");
  const [challenge, setChallenge] = useState<CloudGithubLoginChallenge | null>(null);
  const [channelAvailable, setChannelAvailable] = useState(true);

  const load = useCallback(async () => {
    if (!service) {
      setChannelAvailable(false);
      return;
    }
    setLoading(true);
    try {
      const next = await new Promise<CloudAccountState>((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error("cloud state timeout")), STATE_TIMEOUT_MS);
        service.getState().then(
          (value) => {
            clearTimeout(timer);
            resolve(value);
          },
          (caught: unknown) => {
            clearTimeout(timer);
            reject(caught);
          },
        );
      });
      setState(next);
      setChannelAvailable(true);
      setError("");
    } catch {
      // 旧 Host / Web 没有 cloud-account 频道：隐藏整张卡片，不显示误导性的空状态。
      setChannelAvailable(false);
    } finally {
      setLoading(false);
    }
  }, [service]);

  useEffect(() => {
    if (active) {
      void load();
    }
  }, [active, load]);

  useEffect(() => {
    if (!service) {
      return;
    }
    const subscription = service.onDidChange((next) => {
      setState(next);
      if (next.status === "bound") {
        setChallenge(null);
      }
      if (next.loginError) {
        setError(next.loginError);
        setChallenge(null);
      }
    });
    return () => subscription.dispose();
  }, [service]);

  const message = (id: string, values?: Record<string, string>): string =>
    intl.formatMessage({ id }, values);

  // 未登录且尚未发起授权时自动开始 Device Flow（Host 会打开浏览器并自动轮询）。
  const autoBeginRef = useRef(false);
  useEffect(() => {
    if (!service || !active || challenge || autoBeginRef.current) {
      return;
    }
    if (state?.status !== "unbound") {
      return;
    }
    autoBeginRef.current = true;
    void service
      .beginGithubLogin()
      .then((next) => setChallenge(next))
      .catch((caught: unknown) => {
        setError(caught instanceof Error ? caught.message : String(caught));
      });
  }, [service, active, state?.status, challenge]);

  if (!service || !channelAvailable) {
    return null;
  }

  const connect = async (device: CloudDeviceSummary): Promise<void> => {
    setBusy("connect");
    setError("");
    setConnected("");
    try {
      const result = await service.connectDevice(device.id);
      setConnected(result.name);
      await onPaired();
      await load();
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setBusy(null);
    }
  };

  const beginLogin = async (): Promise<void> => {
    setError("");
    setChallenge(null);
    try {
      setChallenge(await service.beginGithubLogin());
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    }
  };

  const loginPanel = (
    <div className="mt-3 space-y-2" data-testid="remote-control-cloud-login">
      {challenge ? (
        <div className="rounded-lg border border-border bg-background px-3 py-3">
          <div className="flex items-center gap-2 text-ui-base text-foreground">
            <Fingerprint className="size-4 text-foreground-subtle" aria-hidden="true" />
            {message("remoteDesktop.cloud.githubLogin")}
          </div>
          <p className="mt-2 text-ui-base text-foreground-subtle">
            {message("remoteDesktop.cloud.githubCodeTitle")}
          </p>
          <div
            className="mt-2 text-2xl font-semibold tracking-[0.3em] text-foreground"
            data-testid="remote-control-cloud-github-code"
          >
            {challenge.userCode}
          </div>
          {challenge.verificationUri.startsWith("https://github.com/") ? (
            <a
              className="mt-2 inline-flex items-center gap-1 text-ui-base text-sky-600 hover:underline dark:text-sky-400"
              href={challenge.verificationUri}
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
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="mt-2"
            onClick={() => {
              service.cancelGithubLogin();
              setChallenge(null);
            }}
          >
            {message("remoteDesktop.cloud.githubCancel")}
          </Button>
        </div>
      ) : (
        <Button
          type="button"
          variant="outline"
          size="sm"
          data-testid="remote-control-cloud-github-login"
          onClick={() => void beginLogin()}
        >
          <Fingerprint className="size-4" />
          {message("remoteDesktop.cloud.githubLogin")}
        </Button>
      )}
    </div>
  );

  return (
    <section
      className="rounded-xl border border-border bg-card px-4 py-3"
      data-testid="remote-control-cloud"
    >
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <Cloud className="size-4 text-foreground-subtle" aria-hidden="true" />
          <h2 className="text-ui-lg font-medium text-foreground">
            {message("remoteDesktop.cloud.title")}
          </h2>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          disabled={loading || busy !== null}
          onClick={() => {
            setBusy("refresh");
            void load().finally(() => setBusy(null));
          }}
        >
          {busy === "refresh" ? (
            <LoaderIcon className="size-4 animate-spin" />
          ) : (
            <RefreshCw className="size-4" />
          )}
        </Button>
      </div>
      <p className="mt-1 text-ui-base text-foreground-subtle">
        {message("remoteDesktop.cloud.description")}
      </p>

      {loading && !state ? (
        <p className="mt-3 flex items-center gap-2 text-ui-base text-foreground-subtle">
          <LoaderIcon className="size-4 animate-spin" />
          {message("remoteDesktop.cloud.loading")}
        </p>
      ) : null}

      {state?.status === "unavailable" ? (
        <div className="mt-3 space-y-1">
          <p className="text-ui-base text-foreground-subtle">
            {message("remoteDesktop.cloud.unavailable")}
          </p>
          <p className="text-ui-xs text-foreground-subtlest">
            {message("remoteDesktop.cloud.unavailableHint")}
          </p>
        </div>
      ) : null}

      {state?.status === "unbound" ? loginPanel : null}

      {state?.status === "error" ? (
        <div className="mt-3 space-y-2">
          <p className="text-ui-base text-destructive" data-testid="remote-control-cloud-error">
            {error || state.error || message("remoteDesktop.cloud.error")}
          </p>
          {loginPanel}
        </div>
      ) : null}

      {state?.status === "bound" ? (
        <div className="mt-3 space-y-2">
          <p className="text-ui-base text-foreground-subtle">
            {message("remoteDesktop.cloud.account", {
              name:
                state.account?.displayName || state.account?.id || "",
            })}
          </p>
          {connected ? (
            <p
              className="text-ui-base text-foreground"
              data-testid="remote-control-cloud-connected"
            >
              {message("remoteDesktop.cloud.connected", { name: connected })}
            </p>
          ) : null}
          {state.devices && state.devices.length > 0 ? (
            <ul className="space-y-2" data-testid="remote-control-cloud-devices">
              {state.devices.map((device) => (
                <li
                  key={device.id}
                  className="flex items-center justify-between gap-3 rounded-lg border border-border px-3 py-2"
                >
                  <div className="min-w-0">
                    <div className="truncate text-ui-base text-foreground">{device.name}</div>
                    <div className="text-ui-xs text-foreground-subtle">
                      {device.platform}
                      {" · "}
                      {device.online
                        ? message("remoteDesktop.cloud.online")
                        : message("remoteDesktop.cloud.offline")}
                      {!device.accessToken
                        ? ` · ${message("remoteDesktop.cloud.needsRelogin")}`
                        : ""}
                    </div>
                  </div>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    data-testid="remote-control-cloud-connect"
                    disabled={!device.accessToken || !device.online || busy !== null}
                    onClick={() => void connect(device)}
                  >
                    {busy === "connect" ? <LoaderIcon className="size-4 animate-spin" /> : null}
                    {message("remoteDesktop.cloud.connect")}
                  </Button>
                </li>
              ))}
            </ul>
          ) : (
            <p className="text-ui-base text-foreground-subtle">
              {message("remoteDesktop.cloud.noDevices")}
            </p>
          )}

          {error ? <p className="text-ui-base text-destructive">{error}</p> : null}
        </div>
      ) : null}
    </section>
  );
}
