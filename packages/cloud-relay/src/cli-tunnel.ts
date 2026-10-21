import { randomBytes } from "node:crypto";

import { RELAY_TUNNEL_PATH } from "./protocol.js";
import { buildRelayPublicBaseUrl, startRelayTunnelClient } from "./tunnelClient.js";

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

const generate = process.argv.includes("--generate");
const relayUrl = argValue("--relay") ?? process.env.MODE_RELAY_URL ?? "";
const token = argValue("--token") ?? process.env.MODE_RELAY_TOKEN ?? "";
const localBaseUrl =
  argValue("--local") ?? process.env.MODE_RELAY_LOCAL_BASE_URL ?? "http://127.0.0.1:45880";
let tunnelId = argValue("--tunnel-id") ?? process.env.MODE_RELAY_TUNNEL_ID ?? "";

if (!relayUrl.trim()) {
  console.error("--relay / MODE_RELAY_URL is required (e.g. wss://relay.example.com/_tunnel)");
  process.exit(1);
}
if (!token.trim()) {
  console.error("--token / MODE_RELAY_TOKEN is required");
  process.exit(1);
}
if (!tunnelId.trim() || generate) {
  tunnelId = randomBytes(24).toString("base64url");
}

const relay = new URL(relayUrl);
if (relay.protocol !== "ws:" && relay.protocol !== "wss:") {
  console.error("--relay must use ws:// or wss://");
  process.exit(1);
}
if (!relay.pathname || relay.pathname === "/") {
  relay.pathname = RELAY_TUNNEL_PATH;
}

console.log(`[relay-tunnel] tunnelId=${tunnelId}`);
console.log(`[relay-tunnel] public base: ${buildRelayPublicBaseUrl(relay.toString(), tunnelId)}`);
console.log(`[relay-tunnel] local base: ${localBaseUrl}`);

const client = startRelayTunnelClient({
  relayUrl: relay.toString(),
  token,
  tunnelId,
  localBaseUrl,
  log: (line) => console.log(line),
});

const shutdown = (): void => {
  client.close();
  process.exit(0);
};

process.on("SIGINT", shutdown);
process.on("SIGTERM", shutdown);
