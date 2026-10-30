import { mkdirSync } from "node:fs";
import { dirname } from "node:path";

import { startRelayServer } from "./server.js";

function argValue(name: string): string | undefined {
  const args = process.argv.slice(2);
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index]!;
    if (arg === name) {
      return args[index + 1];
    }
    if (arg.startsWith(`${name}=`)) {
      return arg.slice(name.length + 1);
    }
  }
  return undefined;
}

const port = Number(argValue("--port") ?? process.env.MODE_RELAY_PORT ?? "8787");
const host = argValue("--host") ?? process.env.MODE_RELAY_HOST ?? "127.0.0.1";
const token = argValue("--token") ?? process.env.MODE_RELAY_TOKEN ?? "";
const publicBaseUrl =
  argValue("--public-base-url") ??
  process.env.MODE_RELAY_PUBLIC_BASE_URL ??
  `http://localhost:${port}`;
const cloudDbPath =
  argValue("--cloud-db") ?? process.env.MODE_CLOUD_DB_PATH ?? "./data/cloud.sqlite";
const accountPepper =
  argValue("--account-pepper") ?? process.env.MODE_CLOUD_ACCOUNT_PEPPER ?? "";
const dbKey = argValue("--db-key") ?? process.env.MODE_CLOUD_DB_KEY ?? "";
const sessionTtlRaw =
  argValue("--session-ttl-ms") ?? process.env.MODE_CLOUD_SESSION_TTL_MS ?? "";
const githubApiBaseUrl =
  argValue("--github-api-base-url") ?? process.env.MODE_CLOUD_GITHUB_API_BASE_URL ?? "";
const githubClientId =
  argValue("--github-client-id") ?? process.env.MODE_CLOUD_GITHUB_CLIENT_ID ?? "";
const githubBaseUrl =
  argValue("--github-base-url") ?? process.env.MODE_CLOUD_GITHUB_BASE_URL ?? "";
if (cloudDbPath !== ":memory:") {
  mkdirSync(dirname(cloudDbPath), { recursive: true });
}

if (!Number.isInteger(port) || port <= 0 || port > 65535) {
  console.error("invalid --port");
  process.exit(1);
}
if (!token.trim()) {
  console.error("MODE_RELAY_TOKEN / --token is required");
  process.exit(1);
}

const server = await startRelayServer({
  host,
  port,
  token,
  publicBaseUrl,
  log: (line) => console.log(line),
  cloud: {
    dbPath: cloudDbPath,
    accountPepper,
    ...(dbKey.trim() ? { dbKey } : {}),
    ...(sessionTtlRaw.trim() && Number.isFinite(Number(sessionTtlRaw))
      ? { sessionTtlMs: Number(sessionTtlRaw) }
      : {}),
    ...(githubApiBaseUrl.trim() ? { githubApiBaseUrl } : {}),
    ...(githubClientId.trim() ? { githubClientId } : {}),
    ...(githubBaseUrl.trim() ? { githubBaseUrl } : {}),
  },
});

console.log(`[relay] listening on http://${host}:${server.port}`);
console.log(`[relay] public base: ${publicBaseUrl}`);
console.log("[relay] device tunnel: wss://<relay>/_tunnel?tunnelId=<id>&token=<token>");
console.log(`[relay] cloud control: /v1/* (db=${cloudDbPath}, login=github-only)`);
console.log(
  githubClientId.trim()
    ? `[relay] web portal: http://${host}:${server.port}/ (GitHub device flow)`
    : `[relay] web portal: http://${host}:${server.port}/ (github login disabled: no client_id)`,
);

const shutdown = async (): Promise<void> => {
  await server.close();
  process.exit(0);
};

process.on("SIGINT", () => void shutdown());
process.on("SIGTERM", () => void shutdown());
