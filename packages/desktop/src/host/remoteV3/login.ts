import type {
  RemoteV3GithubChallenge,
  RemoteV3HostState,
  RemoteV3PendingLogin,
} from "@mode/shared";

import {
  exchangeGithubAccessToken,
  normalizeRemoteServerUrl,
  pollGithubDeviceFlow,
  startGithubDeviceFlow,
} from "./api.js";

interface PendingGithubLogin {
  serverUrl: string;
  clientId: string;
  deviceCode: string;
  userCode: string;
  verificationUri: string;
  expiresAt: number;
  interval: number;
}

export interface RemoteV3GithubLoginDeps {
  env: NodeJS.ProcessEnv;
  fetchImpl?: typeof fetch;
  getServerUrl: () => string | null;
  configure: (input: { serverUrl: string; webSessionToken: string }) => Promise<RemoteV3HostState>;
  onPendingChange: (pending: RemoteV3PendingLogin | null) => void;
  log: (line: string) => void;
}

/** GitHub Device Flow 状态机：Host 自动轮询，授权成功后注册设备并启动 Control。 */
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
    if (!rawServerUrl.trim()) throw new Error("未配置远程控制 v3 服务地址");
    const serverUrl = normalizeRemoteServerUrl(rawServerUrl);
    const clientId =
      this.deps.env.MODE_REMOTE_V3_GITHUB_CLIENT_ID ??
      this.deps.env.MODE_REMOTE_GITHUB_CLIENT_ID ??
      "";
    if (!clientId) throw new Error("未配置 GitHub client id（MODE_REMOTE_GITHUB_CLIENT_ID）");
    const flow = await startGithubDeviceFlow(clientId, this.deps.fetchImpl);
    this.pending = {
      serverUrl,
      clientId,
      deviceCode: flow.deviceCode,
      userCode: flow.userCode,
      verificationUri: flow.verificationUri,
      expiresAt: Date.now() + flow.expiresIn * 1000,
      interval: flow.interval,
    };
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
      const result = await pollGithubDeviceFlow(
        pending.clientId,
        pending.deviceCode,
        this.deps.fetchImpl,
      );
      if (result.status === "pending") return;
      if (result.status === "slow_down") {
        pending.interval = result.interval ?? pending.interval + 5;
        return;
      }
      if (result.status === "expired") {
        this.clear();
        throw new Error("GitHub 登录已过期，请重新发起");
      }
      const accessToken = result.accessToken;
      if (!accessToken) throw new Error("GitHub 登录响应缺少 access_token");
      const webSessionToken = await exchangeGithubAccessToken({
        serverUrl: pending.serverUrl,
        accessToken,
        fetchImpl: this.deps.fetchImpl,
      });
      const serverUrl = pending.serverUrl;
      this.clear();
      await this.deps.configure({ serverUrl, webSessionToken });
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

  private challenge(): RemoteV3GithubChallenge {
    const pending = this.pending!;
    return {
      userCode: pending.userCode,
      verificationUri: pending.verificationUri,
      expiresIn: Math.max(0, Math.floor((pending.expiresAt - Date.now()) / 1000)),
      interval: pending.interval,
    };
  }

  private pendingView(): RemoteV3PendingLogin {
    const pending = this.pending!;
    return {
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
        this.deps.log(
          `remote_v3_github_poll_failed error=${error instanceof Error ? error.message : String(error)}`,
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
