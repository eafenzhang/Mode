/**
 * Mode Cloud 测试共享助手（docs/specs/cloud-personal-accounts.md，v2）：
 * mock GitHub API、GitHub 会话获取、会话路径 enroll（含 accessToken/addresses）、
 * 目录读取与 withRelay 包装。全部使用 :memory: SQLite 与 mock HTTP。
 * 注意：限速 bucket 按 x-forwarded-for 区分，每个请求用唯一 IP。
 */
import assert from "node:assert/strict";
import { generateKeyPairSync, randomBytes, sign, type KeyObject } from "node:crypto";
import { createServer, type Server } from "node:http";

import { startRelayServer, type RelayServer } from "../src/server.js";

export const RELAY_TOKEN = "relay-token";
export const PUBLIC_BASE = "https://relay.example.test";
export const DB_KEY = "test-db-key";

let ipCounter = 0;
export function nextIp(): string {
  ipCounter += 1;
  return `10.1.0.${ipCounter}`;
}

export interface MockGithubOptions {
  /** 返回的 GitHub 数字用户 id 与 login；token → 用户映射。 */
  tokens?: Record<string, { id: number; login: string; name?: string }>;
}

export interface MockGithub {
  server: Server;
  port: number;
  close(): Promise<void>;
}

/** 启动 mock GitHub API：`GET /user` 按 Bearer token 返回用户或 401。 */
export async function startMockGithub(options: MockGithubOptions = {}): Promise<MockGithub> {
  const tokens = options.tokens ?? {};
  const server = createServer((req, res) => {
    req.on("data", () => undefined);
    req.on("end", () => {
      const auth = String(req.headers.authorization ?? "").replace(/^Bearer /u, "");
      const profile = tokens[auth];
      res.writeHead(profile ? 200 : 401, { "content-type": "application/json" });
      res.end(
        JSON.stringify(
          profile
            ? { id: profile.id, login: profile.login, ...(profile.name ? { name: profile.name } : {}) }
            : { message: "Bad credentials" },
        ),
      );
    });
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  return {
    server,
    port,
    close: async () => {
      await new Promise<void>((resolve) => server.close(() => resolve()));
    },
  };
}

export async function withRelay(
  cloud: Record<string, unknown>,
  run: (relay: RelayServer) => Promise<void>,
): Promise<void> {
  const relay = await startRelayServer({
    port: 0,
    token: RELAY_TOKEN,
    publicBaseUrl: PUBLIC_BASE,
    log: () => undefined,
    cloud: {
      dbPath: ":memory:",
      accountPepper: "pepper",
      // 测试用固定密钥，避免 secretBox 在测试目录落 cloud-db.key。
      dbKey: DB_KEY,
      ...cloud,
    },
  });
  try {
    await run(relay);
  } finally {
    await relay.close();
  }
}

export interface AuthResponse {
  status: number;
  body: Record<string, unknown>;
  setCookie?: string;
}

export async function postJson(
  port: number,
  path: string,
  body: unknown,
  token?: string,
  headers: Record<string, string> = {},
): Promise<AuthResponse> {
  const response = await fetch(`http://127.0.0.1:${port}${path}`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-forwarded-for": nextIp(),
      ...(token ? { authorization: `Bearer ${token}` } : {}),
      ...headers,
    },
    body: JSON.stringify(body),
  });
  const parsed = (await response.json().catch(() => ({}))) as Record<string, unknown>;
  const setCookie = response.headers.get("set-cookie") ?? undefined;
  return { status: response.status, body: parsed, ...(setCookie ? { setCookie } : {}) };
}

export async function githubSession(
  relay: RelayServer,
  githubToken: string,
): Promise<{ sessionToken: string; accountId: string; displayName?: string }> {
  const result = await postJson(relay.port, "/v1/auth/github", {
    accessToken: githubToken,
    clientLabel: "test",
  });
  assert.equal(result.status, 200, `github login failed: ${JSON.stringify(result.body)}`);
  const account = result.body.account as Record<string, unknown>;
  return {
    sessionToken: String(result.body.sessionToken),
    accountId: String(account.id),
    ...(typeof account.displayName === "string" ? { displayName: account.displayName } : {}),
  };
}

export function createKeyPair(): { privateKey: KeyObject; publicKey: KeyObject } {
  return generateKeyPairSync("ed25519");
}

export interface EnrollSessionOptions {
  deviceId: string;
  tunnelId: string;
  accessToken?: string;
  addresses?: string[];
  name?: string;
}

/** 会话路径 enroll：Bearer sessionToken，断言 payload 不含 account 块。 */
export async function enrollWithSession(
  port: number,
  sessionToken: string,
  options: EnrollSessionOptions,
): Promise<AuthResponse> {
  const { privateKey, publicKey } = createKeyPair();
  const issuedAt = Date.now();
  const nonce = randomBytes(16).toString("base64url");
  const payload = JSON.stringify({
    device: { id: options.deviceId, relayTunnelId: options.tunnelId },
    nonce,
    issuedAt,
  });
  const signature = sign(null, Buffer.from(payload, "utf8"), privateKey).toString("base64");
  const accessToken = options.accessToken ?? "device-access-token-0123456789";
  const response = await fetch(`http://127.0.0.1:${port}/v1/devices/enroll`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      authorization: `Bearer ${sessionToken}`,
      "x-forwarded-for": nextIp(),
    },
    body: JSON.stringify({
      device: {
        id: options.deviceId,
        name: options.name ?? "Desk",
        platform: "win32",
        version: "1.0.0",
        publicKey: publicKey.export({ type: "spki", format: "pem" }).toString(),
        relayTunnelId: options.tunnelId,
        accessToken,
        ...(options.addresses ? { addresses: options.addresses } : {}),
      },
      assertion: { payload, signature },
      nonce,
      issuedAt,
    }),
  });
  return {
    status: response.status,
    body: (await response.json().catch(() => ({}))) as Record<string, unknown>,
  };
}

export async function listDevices(port: number, token: string): Promise<AuthResponse> {
  const response = await fetch(`http://127.0.0.1:${port}/v1/devices`, {
    headers: { authorization: `Bearer ${token}` },
  });
  return {
    status: response.status,
    body: (await response.json().catch(() => ({}))) as Record<string, unknown>,
  };
}
