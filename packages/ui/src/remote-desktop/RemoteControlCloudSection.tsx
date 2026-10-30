import { useCallback, useEffect, useState } from "react";
import { Cloud, Fingerprint, KeyRound, LoaderIcon, RefreshCw } from "lucide-react";
import type { CloudAccountState, CloudDeviceSummary } from "@mode/shared";
import { Button } from "@/ui-foundation/adapters/button.js";
import { Input } from "@/ui-foundation/adapters/input.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { useServices } from "@/hooks/useServices.js";
import { useOptionalBaseWorkspaceServices } from "@/hooks/useWorkspaceServices.js";
import { RemoteControlCloudGithubPanel } from "@/remote-desktop/RemoteControlCloudGithubPanel.js";
import {
  RemoteControlCloudPasswordPanel,
  type CloudPasswordPanelMode,
} from "@/remote-desktop/RemoteControlCloudPasswordPanel.js";

// 目录请求本身有 10s 预算；前端等待必须覆盖它，否则慢网会被误判为“频道不存在”而隐藏卡片。
const STATE_TIMEOUT_MS = 12_000;

/**
 * Mode Cloud 个人账号与设备目录（docs/specs/cloud-personal-accounts.md）。
 *
 * 登录方式：GitHub Device Flow 或用户名密码；目录只提供被控端公网地址，
 * 6 位码仍由被控端显示/裁决。配对成功后复用既有 LAN 配对列表。
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
  const [busy, setBusy] = useState<"refresh" | "pair" | null>(null);
  const [error, setError] = useState("");
  const [pairTarget, setPairTarget] = useState<CloudDeviceSummary | null>(null);
  const [code, setCode] = useState("");
  const [channelAvailable, setChannelAvailable] = useState(true);
  const [showGithubLogin, setShowGithubLogin] = useState(false);
  const [showPasswordPanel, setShowPasswordPanel] = useState<CloudPasswordPanelMode | null>(null);

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
    const subscription = service.onDidChange((next) => setState(next));
    return () => subscription.dispose();
  }, [service]);

  if (!service || !channelAvailable) {
    return null;
  }

  const pair = async (): Promise<void> => {
    if (!pairTarget) {
      return;
    }
    setBusy("pair");
    setError("");
    try {
      await service.pairDevice({ deviceId: pairTarget.id, code });
      setPairTarget(null);
      setCode("");
      await onPaired();
      await load();
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setBusy(null);
    }
  };

  const message = (id: string, values?: Record<string, string>): string =>
    intl.formatMessage({ id }, values);

  const loginButtons = (
    <div className="flex flex-wrap items-center gap-2">
      <Button
        type="button"
        variant="outline"
        size="sm"
        data-testid="remote-control-cloud-github-login"
        disabled={showGithubLogin || showPasswordPanel !== null}
        onClick={() => setShowGithubLogin(true)}
      >
        <Fingerprint className="size-4" />
        {message("remoteDesktop.cloud.githubLogin")}
      </Button>
      <Button
        type="button"
        variant="ghost"
        size="sm"
        data-testid="remote-control-cloud-password-login"
        disabled={showGithubLogin || showPasswordPanel !== null}
        onClick={() => setShowPasswordPanel("login")}
      >
        <KeyRound className="size-4" />
        {message("remoteDesktop.cloud.passwordLogin")}
      </Button>
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

      {state?.status === "unbound" ? (
        <div className="mt-3 space-y-2">
          {loginButtons}
          {showGithubLogin ? (
            <RemoteControlCloudGithubPanel
              service={service}
              onBound={(next) => {
                setState(next);
                setShowGithubLogin(false);
              }}
              onClose={() => setShowGithubLogin(false)}
            />
          ) : null}
          {showPasswordPanel ? (
            <RemoteControlCloudPasswordPanel
              service={service}
              mode={showPasswordPanel}
              onDone={(next) => {
                setState(next);
                setShowPasswordPanel(null);
              }}
              onClose={() => setShowPasswordPanel(null)}
            />
          ) : null}
        </div>
      ) : null}

      {state?.status === "error" ? (
        <div className="mt-3 space-y-2">
          <p className="text-ui-base text-destructive" data-testid="remote-control-cloud-error">
            {error || state.error || message("remoteDesktop.cloud.error")}
          </p>
          {loginButtons}
        </div>
      ) : null}

      {state?.status === "bound" ? (
        <div className="mt-3 space-y-2">
          <div className="flex flex-wrap items-center gap-2">
            <p className="text-ui-base text-foreground-subtle">
              {message("remoteDesktop.cloud.account", {
                name:
                  state.account?.displayName ||
                  state.account?.username ||
                  state.account?.id ||
                  "",
              })}
            </p>
            {!state.account?.username ? (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                data-testid="remote-control-cloud-set-credentials"
                disabled={showPasswordPanel !== null}
                onClick={() => setShowPasswordPanel("credentials")}
              >
                {message("remoteDesktop.cloud.setCredentials")}
              </Button>
            ) : null}
          </div>
          {showPasswordPanel ? (
            <RemoteControlCloudPasswordPanel
              service={service}
              mode={showPasswordPanel}
              onDone={(next) => {
                setState(next);
                setShowPasswordPanel(null);
              }}
              onClose={() => setShowPasswordPanel(null)}
            />
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
                    </div>
                  </div>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    disabled={!device.online || busy !== null}
                    onClick={() => {
                      setPairTarget(device);
                      setCode("");
                      setError("");
                    }}
                  >
                    {message("remoteDesktop.cloud.pair")}
                  </Button>
                </li>
              ))}
            </ul>
          ) : (
            <p className="text-ui-base text-foreground-subtle">
              {message("remoteDesktop.cloud.noDevices")}
            </p>
          )}

          {pairTarget ? (
            <div className="mt-2 rounded-lg border border-border bg-background px-3 py-3">
              <p className="text-ui-base text-foreground">
                {message("remoteDesktop.cloud.pairTitle", { name: pairTarget.name })}
              </p>
              <div className="mt-2 flex flex-wrap items-center gap-2">
                <Input
                  value={code}
                  maxLength={8}
                  className="w-32 tracking-[0.3em] uppercase"
                  placeholder={message("remoteDesktop.cloud.codePlaceholder")}
                  onChange={(event) => {
                    setCode(event.target.value);
                    setError("");
                  }}
                />
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={busy !== null || !code.trim()}
                  onClick={() => void pair()}
                >
                  {busy === "pair" ? <LoaderIcon className="size-4 animate-spin" /> : null}
                  {message(
                    busy === "pair"
                      ? "remoteDesktop.cloud.pairing"
                      : "remoteDesktop.cloud.pairSubmit",
                  )}
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  disabled={busy === "pair"}
                  onClick={() => {
                    setPairTarget(null);
                    setCode("");
                  }}
                >
                  {message("remoteDesktop.cloud.cancel")}
                </Button>
              </div>
              <p className="mt-1 text-ui-xs text-foreground-subtlest">
                {message("remoteDesktop.cloud.pairHint")}
              </p>
            </div>
          ) : null}

          {error ? <p className="text-ui-base text-destructive">{error}</p> : null}
        </div>
      ) : null}
    </section>
  );
}
