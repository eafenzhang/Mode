import { useEffect, useState } from "react";
import type { WebAccountClient, WebDeviceProfile, WebGithubChallenge } from "@mode/remote-core/web";

const PENDING_KEY = "mode-v3-oauth-pending";
const POLL_INTERVAL_MS = 2_000;

interface PendingOAuth {
  serverUrl: string;
  state: string;
  pollToken: string;
  expiresAt: number;
}

/** 发起前把 state/pollToken 存在本标签页；回调返回后用它换取会话。 */
export function saveV3OAuthPending(
  serverUrl: string,
  challenge: Extract<WebGithubChallenge, { kind: "oauth" }>,
): void {
  const pending: PendingOAuth = {
    serverUrl,
    state: challenge.state,
    pollToken: challenge.pollToken,
    expiresAt: Date.now() + challenge.expiresIn * 1000,
  };
  try {
    sessionStorage.setItem(PENDING_KEY, JSON.stringify(pending));
  } catch {
    // 隐私模式禁用存储时仍跳转授权，返回后提示重新登录。
  }
}

/**
 * GitHub OAuth 回调返回 `/v3?oauth=1` 时完成登录：用会话存储里的 pollToken
 * 轮询服务端，成功后写回 profile 与设备目录，并清理 URL 参数。
 */
export function useV3OAuthReturn(
  client: WebAccountClient | null,
  setProfile: (profile: WebDeviceProfile) => void,
  refreshHosts: () => Promise<void>,
  onError: (message: string) => void,
): boolean {
  const [processing, setProcessing] = useState(false);

  useEffect(() => {
    if (!client) return;
    const params = new URLSearchParams(window.location.search);
    const callback = params.get("oauth");
    if (callback !== "1" && callback !== "error") return;
    const pending = readPending();
    clearPending();
    const cleanUrl = (): void => {
      window.history.replaceState(null, "", `${window.location.pathname}${window.location.hash}`);
    };
    cleanUrl();
    if (!pending) {
      onError("登录结果已丢失，请重新发起 GitHub 登录");
      return;
    }
    if (callback === "error") {
      onError("GitHub 授权被拒绝");
      return;
    }
    let cancelled = false;
    setProcessing(true);
    void (async () => {
      try {
        while (!cancelled && Date.now() < pending.expiresAt) {
          const result = await client.pollGithubOAuthLogin(pending.state, pending.pollToken);
          if (cancelled) return;
          if (result.status === "ok") {
            setProfile(result.profile);
            await refreshHosts();
            return;
          }
          if (result.status === "pending") {
            await new Promise((resolve) => setTimeout(resolve, POLL_INTERVAL_MS));
            continue;
          }
          if (result.status === "expired") {
            onError("GitHub 登录已过期，请重新发起");
            return;
          }
          if (result.status === "denied") {
            onError("GitHub 授权被拒绝");
            return;
          }
          onError(result.status === "error" ? result.message : "GitHub 登录失败");
          return;
        }
        if (!cancelled) onError("GitHub 登录超时，请重新发起");
      } catch (caught) {
        if (!cancelled) onError(caught instanceof Error ? caught.message : String(caught));
      } finally {
        if (!cancelled) setProcessing(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [client, onError, refreshHosts, setProfile]);

  return processing;
}

function readPending(): PendingOAuth | null {
  try {
    const raw = sessionStorage.getItem(PENDING_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<PendingOAuth>;
    if (
      typeof parsed.serverUrl !== "string" ||
      typeof parsed.state !== "string" ||
      typeof parsed.pollToken !== "string" ||
      typeof parsed.expiresAt !== "number"
    ) {
      return null;
    }
    return parsed as PendingOAuth;
  } catch {
    return null;
  }
}

function clearPending(): void {
  try {
    sessionStorage.removeItem(PENDING_KEY);
  } catch {
    // 忽略存储不可用。
  }
}
