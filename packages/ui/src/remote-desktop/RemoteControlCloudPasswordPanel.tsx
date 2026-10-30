import { useState } from "react";
import { KeyRound, LoaderIcon } from "lucide-react";
import type { CloudAccountState } from "@mode/shared";
import type { ICloudAccountService } from "@mode/services";
import { Button } from "@/ui-foundation/adapters/button.js";
import { Input } from "@/ui-foundation/adapters/input.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";

export type CloudPasswordPanelMode = "login" | "register" | "credentials";

/**
 * 用户名密码登录/注册/补设面板（docs/specs/cloud-personal-accounts.md）。
 * credentials 模式给已绑定但未设密码的账号（GitHub/迁移账号）补设备用登录方式。
 */
export function RemoteControlCloudPasswordPanel({
  service,
  mode: initialMode,
  onDone,
  onClose,
}: {
  service: ICloudAccountService;
  mode: CloudPasswordPanelMode;
  onDone: (state: CloudAccountState) => void;
  onClose: () => void;
}) {
  const { intl } = useModeIntl();
  const [mode, setMode] = useState<CloudPasswordPanelMode>(initialMode);
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [inviteCode, setInviteCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const message = (id: string): string => intl.formatMessage({ id });
  const usernameValid = /^[a-z0-9_-]{3,32}$/u.test(username.trim());
  const passwordValid = password.length >= 8;
  // 邀请码不阻塞提交：部署方配置了邀请码时由服务端 403 提示，避免误判部署策略。
  const canSubmit = !busy && usernameValid && passwordValid;

  const submit = async (): Promise<void> => {
    setBusy(true);
    setError("");
    try {
      if (mode === "login") {
        onDone(await service.loginWithPassword({ username: username.trim(), password }));
      } else if (mode === "register") {
        onDone(
          await service.registerWithPassword({
            username: username.trim(),
            password,
            ...(inviteCode.trim() ? { inviteCode: inviteCode.trim() } : {}),
          }),
        );
      } else {
        onDone(await service.setAccountCredentials({ username: username.trim(), password }));
      }
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="mt-2 rounded-lg border border-border bg-background px-3 py-3">
      <div className="flex items-center gap-2 text-ui-base text-foreground">
        <KeyRound className="size-4 text-foreground-subtle" aria-hidden="true" />
        {message(
          mode === "login"
            ? "remoteDesktop.cloud.passwordLogin"
            : mode === "register"
              ? "remoteDesktop.cloud.passwordRegister"
              : "remoteDesktop.cloud.setCredentials",
        )}
      </div>
      {mode === "credentials" ? (
        <p className="mt-1 text-ui-xs text-foreground-subtlest">
          {message("remoteDesktop.cloud.setCredentialsHint")}
        </p>
      ) : null}
      <div className="mt-2 space-y-2">
        <Input
          value={username}
          className="w-64"
          autoComplete="username"
          aria-label={message("remoteDesktop.cloud.passwordUsername")}
          placeholder={message("remoteDesktop.cloud.passwordUsername")}
          data-testid="remote-control-cloud-username-input"
          onChange={(event) => {
            setUsername(event.target.value);
            setError("");
          }}
        />
        <p className="text-ui-xs text-foreground-subtlest">
          {message("remoteDesktop.cloud.passwordUsernameHint")}
        </p>
        <Input
          value={password}
          type="password"
          className="w-64"
          autoComplete={mode === "login" ? "current-password" : "new-password"}
          aria-label={message("remoteDesktop.cloud.passwordPassword")}
          placeholder={message("remoteDesktop.cloud.passwordPassword")}
          data-testid="remote-control-cloud-password-input"
          onChange={(event) => {
            setPassword(event.target.value);
            setError("");
          }}
        />
        <p className="text-ui-xs text-foreground-subtlest">
          {message("remoteDesktop.cloud.passwordPasswordHint")}
        </p>
        {mode === "register" ? (
          <Input
            value={inviteCode}
            className="w-64"
            aria-label={message("remoteDesktop.cloud.passwordInvite")}
            placeholder={message("remoteDesktop.cloud.passwordInvite")}
            data-testid="remote-control-cloud-invite-input"
            onChange={(event) => {
              setInviteCode(event.target.value);
              setError("");
            }}
          />
        ) : null}
      </div>
      {error ? <p className="mt-2 text-ui-base text-destructive">{error}</p> : null}
      <div className="mt-3 flex flex-wrap items-center gap-2">
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={!canSubmit}
          data-testid="remote-control-cloud-password-submit"
          onClick={() => void submit()}
        >
          {busy ? <LoaderIcon className="size-4 animate-spin" /> : null}
          {message(
            mode === "login"
              ? "remoteDesktop.cloud.passwordSubmitLogin"
              : mode === "register"
                ? "remoteDesktop.cloud.passwordSubmitRegister"
                : "remoteDesktop.cloud.passwordSubmitCredentials",
          )}
        </Button>
        {mode !== "credentials" ? (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            disabled={busy}
            onClick={() => {
              setError("");
              setMode(mode === "login" ? "register" : "login");
            }}
          >
            {message(
              mode === "login"
                ? "remoteDesktop.cloud.passwordToggleRegister"
                : "remoteDesktop.cloud.passwordToggleLogin",
            )}
          </Button>
        ) : null}
        <Button type="button" variant="ghost" size="sm" disabled={busy} onClick={onClose}>
          {message("remoteDesktop.cloud.close")}
        </Button>
      </div>
    </div>
  );
}
