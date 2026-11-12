import type {
  RemoteV3GithubChallenge,
  RemoteV3HostState,
  RemoteV3PendingLogin,
} from "@mode/shared";

import {
  GithubOAuthUnavailableError,
  normalizeRemoteServerUrl,
  pollGithubDeviceFlow,
  pollGithubOAuthFlow,
  startGithubDeviceFlow,
  startGithubOAuthFlow,
} from "./api.js";

type PendingGithubLogin =
  | {
      kind: "device";
      serverUrl: string;
      deviceCode: string;
      userCode: string;
      verificationUri: string;
      expiresAt: number;
      interval: number;
    }
  | {
      kind: "oauth";
      serverUrl: string;
      state: string;
      pollToken: string;
      authorizationUrl: string;
      expiresAt: number;
      interval: number;
    };

export interface RemoteV3GithubLoginDeps {
  env: NodeJS.ProcessEnv;
  fetchImpl?: typeof fetch;
  getServerUrl: () => string | null;
  configure: (input: { serverUrl: string; webSessionToken: string }) => Promise<RemoteV3HostState>;
  onPendingChange: (pending: RemoteV3PendingLogin | null) => void;
  log: (line: string) => void;
}

/**
 * GitHub 登录状态机：优先 OAuth App 回调（一键授权，无验证码），服务端未配置时
 * 回落 Device Flow。两种流程都由 Host 自动轮询，授权成功后注册设备并启动 Control。
 */
export class RemoteV3GithubLogin {
  private readonly deps: RemoteV3GithubLoginDeps;
  private pending: PendingGithubLogin | null = null;
  private pollTimer: NodeJS.Timeout | null = null;
  private busy = false;

  constructor(deps: RemoteV3GithubLoginDeps) {
    this.deps = deps;
  }

  async begin(): Promise<RemoteV3GithubChallenge> {
    if (this.pending) return this.challenge();
    const rawServerUrl = this.deps.getServerUrl() ?? this.deps.env.MODE_REMOTE_V3_URL ?? "";
    if (!rawServerUrl.trim()) throw new Error("未配置远程控制服务地址");
    const serverUrl = normalizeRemoteServerUrl(rawServerUrl);
    try {
      const flow = await startGithubOAuthFlow({
        serverUrl,
        ...(this.deps.fetchImpl ? { fetchImpl: this.deps.fetchImpl } : {}),
      });
      this.pending = {
        kind: "oauth",
        serverUrl,
        state: flow.state,
        pollToken: flow.pollToken,
        authorizationUrl: flow.authorizationUrl,
        expiresAt: Date.now() + flow.expiresIn * 1000,
        interval: flow.interval,
      };
    } catch (error) {
      if (!(error instanceof GithubOAuthUnavailableError)) throw error;
      const flow = await startGithubDeviceFlow({
        serverUrl,
        ...(this.deps.fetchImpl ? { fetchImpl: this.deps.fetchImpl } : {}),
      });
      this.pending = {
        kind: "device",
        serverUrl,
        deviceCode: flow.deviceCode,
        userCode: flow.userCode,
        verificationUri: flow.verificationUri,
        expiresAt: Date.now() + flow.expiresIn * 1000,
        interval: flow.interval,
      };
    }
    this.deps.onPendingChange(this.pendingView());
    this.schedule();
    return this.challenge();
  }

  async poll(): Promise<void> {
    const pending = this.pending;
    if (!pending) throw new Error("没有等待中的 GitHub 登录");
    if (pending.expiresAt <= Date.now()) {
      this.clear();
      throw new Error("GitHub 登录已过期，请重新发起");
    }
    if (this.busy) return;
    this.busy = true;
    try {
      if (pending.kind === "oauth") {
        const result = await pollGithubOAuthFlow({
          serverUrl: pending.serverUrl,
          state: pending.state,
          pollToken: pending.pollToken,
          ...(this.deps.fetchImpl ? { fetchImpl: this.deps.fetchImpl } : {}),
        });
        if (result.status === "pending") return;
        if (result.status === "expired") {
          this.clear();
          throw new Error("GitHub 登录已过期，请重新发起");
        }
        if (result.status === "denied") {
          this.clear();
          throw new Error("GitHub 授权被拒绝");
        }
        if (result.status === "error") {
          this.clear();
          throw new Error(result.message ?? "GitHub 授权失败");
        }
        await this.finishLogin(pending.serverUrl, result.sessionToken);
        return;
      }
      const result = await pollGithubDeviceFlow({
        serverUrl: pending.serverUrl,
        deviceCode: pending.deviceCode,
        ...(this.deps.fetchImpl ? { fetchImpl: this.deps.fetchImpl } : {}),
      });
      if (result.status === "pending") return;
      if (result.status === "slow_down") {
        pending.interval = result.interval ?? pending.interval + 5;
        return;
      }
      if (result.status === "expired") {
        this.clear();
        throw new Error("GitHub 登录已过期，请重新发起");
      }
      if (result.status === "denied") {
        this.clear();
        throw new Error("GitHub 授权被拒绝");
      }
      await this.finishLogin(pending.serverUrl, result.sessionToken);
    } finally {
      this.busy = false;
      if (this.pending) this.schedule();
    }
  }

  cancel(): void {
    this.clear();
  }

  dispose(): void {
    this.clear();
  }

  private async finishLogin(serverUrl: string, webSessionToken: string | undefined): Promise<void> {
    if (!webSessionToken) throw new Error("GitHub 登录响应缺少 sessionToken");
    this.clear();
    await this.deps.configure({ serverUrl, webSessionToken });
  }

  private challenge(): RemoteV3GithubChallenge {
    const pending = this.pending!;
    const expiresIn = Math.max(0, Math.floor((pending.expiresAt - Date.now()) / 1000));
    return pending.kind === "oauth"
      ? {
          kind: "oauth",
          authorizationUrl: pending.authorizationUrl,
          expiresIn,
          interval: pending.interval,
        }
      : {
          kind: "device",
          userCode: pending.userCode,
          verificationUri: pending.verificationUri,
          expiresIn,
          interval: pending.interval,
        };
  }

  private pendingView(): RemoteV3PendingLogin {
    const pending = this.pending!;
    return pending.kind === "oauth"
      ? { kind: "oauth", authorizationUrl: pending.authorizationUrl, expiresAt: pending.expiresAt }
      : {
          kind: "device",
          userCode: pending.userCode,
          verificationUri: pending.verificationUri,
          expiresAt: pending.expiresAt,
        };
  }

  private schedule(): void {
    this.clearTimer();
    if (!this.pending) return;
    this.pollTimer = setTimeout(() => {
      void this.poll().catch((error: unknown) => {
        const cause =
          error instanceof Error && error.cause
            ? ((error.cause as { code?: string; message?: string }).code ??
              (error.cause as { message?: string }).message ??
              String(error.cause))
            : "";
        this.deps.log(
          `remote_v3_github_poll_failed error=${error instanceof Error ? error.message : String(error)}${cause ? ` cause=${cause}` : ""}`,
        );
      });
    }, this.pending.interval * 1000);
  }

  private clear(): void {
    this.pending = null;
    this.clearTimer();
    this.deps.onPendingChange(null);
  }

  private clearTimer(): void {
    if (this.pollTimer) clearTimeout(this.pollTimer);
    this.pollTimer = null;
  }
}
