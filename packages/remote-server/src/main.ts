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

const handle = await startRemoteServer({
  port,
  host: process.env.MODE_REMOTE_HOST ?? "0.0.0.0",
  secret,
  dbPath,
  pepper,
  publicUrl,
  allowedOrigins: process.env.MODE_REMOTE_ALLOWED_ORIGINS
    ?.split(",")
    .map((value) => value.trim())
    .filter(Boolean),
  githubVerifier: githubApiBaseUrl
    ? (await import("./github.js")).createGithubVerifier({ apiBaseUrl: githubApiBaseUrl })
    : undefined,
  ...(tls ? { tls } : {}),
  ...(metricsToken ? { metricsToken } : {}),
  ...(githubClientId ? { githubClientId } : {}),
});

const shutdown = async (): Promise<void> => {
  await handle.close();
  process.exit(0);
};
process.on("SIGINT", () => void shutdown());
process.on("SIGTERM", () => void shutdown());
