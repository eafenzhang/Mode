import { readFile } from "node:fs/promises";

import { startRemoteServer } from "./server.js";

const port = Number.parseInt(process.env.MODE_REMOTE_PORT ?? "45890", 10);
const dbPath = process.env.MODE_REMOTE_DB ?? "./data/remote-v3.sqlite";
const secret = process.env.MODE_REMOTE_SECRET ?? "";
const pepper = process.env.MODE_REMOTE_PEPPER;
const publicUrl = process.env.MODE_REMOTE_PUBLIC_URL;
const githubApiBaseUrl = process.env.MODE_GITHUB_API_BASE_URL;
const tlsKeyFile = process.env.MODE_REMOTE_TLS_KEY_FILE?.trim();
const tlsCertFile = process.env.MODE_REMOTE_TLS_CERT_FILE?.trim();
const metricsToken = process.env.MODE_REMOTE_METRICS_TOKEN?.trim();
const githubClientId = process.env.MODE_GITHUB_CLIENT_ID?.trim();
const githubClientSecret = process.env.MODE_GITHUB_CLIENT_SECRET?.trim();
const githubOAuthCallbackUrl = process.env.MODE_GITHUB_OAUTH_CALLBACK_URL?.trim();
const githubOAuthAuthorizeBaseUrl = process.env.MODE_GITHUB_OAUTH_AUTHORIZE_BASE_URL?.trim();
const githubOAuthTokenBaseUrl = process.env.MODE_GITHUB_OAUTH_TOKEN_BASE_URL?.trim();
// github.com 的 Device Flow / OAuth token 端点走代理（限制网络/自建反代）；GitHub API 仍直连。
const githubDeviceBaseUrl = process.env.MODE_GITHUB_DEVICE_BASE_URL?.trim();

if (!secret) {
  console.error("MODE_REMOTE_SECRET 不能为空");
  process.exit(1);
}
if (Boolean(tlsKeyFile) !== Boolean(tlsCertFile)) {
  console.error("MODE_REMOTE_TLS_KEY_FILE 与 MODE_REMOTE_TLS_CERT_FILE 必须同时提供");
  process.exit(1);
}
const tls = tlsKeyFile
  ? { key: await readFile(tlsKeyFile), cert: await readFile(tlsCertFile!) }
  : undefined;

const allowedOrigins = (process.env.MODE_REMOTE_ALLOWED_ORIGINS ?? "")
  .split(",")
  .map((value) => value.trim())
  .filter(Boolean);

const handle = await startRemoteServer({
  port,
  host: process.env.MODE_REMOTE_HOST ?? "0.0.0.0",
  secret,
  dbPath,
  pepper,
  publicUrl,
  webrtc: buildWebRtcConfigFromEnv(),
  ...(allowedOrigins.length > 0 ? { allowedOrigins } : {}),
  githubVerifier: githubApiBaseUrl
    ? (await import("./github.js")).createGithubVerifier({ apiBaseUrl: githubApiBaseUrl })
    : undefined,
  ...(tls ? { tls } : {}),
  ...(metricsToken ? { metricsToken } : {}),
  ...(githubClientId ? { githubClientId } : {}),
  ...(githubClientSecret ? { githubClientSecret } : {}),
  ...(githubOAuthCallbackUrl ? { githubOAuthCallbackUrl } : {}),
  ...(githubOAuthAuthorizeBaseUrl ? { githubOAuthAuthorizeBaseUrl } : {}),
  ...(githubOAuthTokenBaseUrl ? { githubOAuthTokenBaseUrl } : {}),
  ...(githubDeviceBaseUrl ? { githubDeviceBaseUrl } : {}),
});

const shutdown = async (): Promise<void> => {
  await handle.close();
  process.exit(0);
};
process.on("SIGINT", () => void shutdown());
process.on("SIGTERM", () => void shutdown());

/** 从环境变量读取 WebRTC ICE 配置；未配置 TURN 时只下发 STUN。 */
function buildWebRtcConfigFromEnv(): import("./gatewayTypes.js").WebRtcConfig | undefined {
  const stunUrls = (process.env.MODE_REMOTE_STUN_URLS ?? "")
    .split(",")
    .map((value) => value.trim())
    .filter(Boolean);
  const turnUrls = (process.env.MODE_REMOTE_TURN_URLS ?? "")
    .split(",")
    .map((value) => value.trim())
    .filter(Boolean);
  const turnSecret = process.env.MODE_REMOTE_TURN_SECRET?.trim() ?? "";
  if (stunUrls.length === 0 && turnUrls.length === 0) return undefined;
  return {
    ...(stunUrls.length > 0 ? { stunUrls } : {}),
    ...(turnUrls.length > 0 && turnSecret
      ? {
          turn: {
            urls: turnUrls,
            secret: turnSecret,
            ttlSeconds: Number(process.env.MODE_REMOTE_TURN_TTL_SECONDS ?? "3600") || 3600,
          },
        }
      : {}),
  };
}
