import { createHmac } from "node:crypto";

import { StoreError } from "./store.js";

export interface GitHubIdentity {
  providerUserId: string;
  login?: string;
  name?: string;
}

export type GitHubVerifier = (accessToken: string) => Promise<GitHubIdentity>;

export interface GithubVerifierOptions {
  apiBaseUrl?: string;
  fetchImpl?: typeof fetch;
}

/** GitHub OAuth Device Flow 的 access token 只用于一次性 GET /user，用完即弃。 */
export function createGithubVerifier(options: GithubVerifierOptions = {}): GitHubVerifier {
  const apiBaseUrl = (options.apiBaseUrl ?? "https://api.github.com").replace(/\/+$/, "");
  const fetchImpl = options.fetchImpl ?? fetch;
  return async (accessToken: string) => {
    if (accessToken.length < 16 || accessToken.length > 512) {
      throw new StoreError("invalid_github_token", "GitHub token 格式非法");
    }
    let response: Response;
    try {
      response = await fetchImpl(`${apiBaseUrl}/user`, {
        headers: {
          authorization: `Bearer ${accessToken}`,
          accept: "application/vnd.github+json",
          "user-agent": "mode-remote/3",
        },
      });
    } catch {
      throw new StoreError("github_unreachable", "GitHub API 不可达");
    }
    if (response.status === 401) {
      throw new StoreError("invalid_github_token", "GitHub token 无效");
    }
    if (!response.ok) {
      throw new StoreError("github_unreachable", `GitHub API 返回 ${response.status}`);
    }
    const body = (await response.json()) as {
      id?: unknown;
      login?: unknown;
      name?: unknown;
    };
    if (typeof body.id !== "number" || !Number.isSafeInteger(body.id)) {
      throw new StoreError("github_unreachable", "GitHub 用户响应缺少 id");
    }
    return {
      providerUserId: String(body.id),
      login: typeof body.login === "string" ? body.login : undefined,
      name: typeof body.name === "string" ? body.name : undefined,
    };
  };
}

export function accountAnchor(pepper: string, providerUserId: string): string {
  return createHmac("sha256", pepper).update(`github:${providerUserId}`).digest("hex");
}
