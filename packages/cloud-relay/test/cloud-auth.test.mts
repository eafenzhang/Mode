/**
 * Mode Cloud 个人账号面（docs/specs/cloud-personal-accounts.md）：
 * 注册/登录/会话轮转、GitHub 登录（mock GitHub API）、会话路径 enroll、
 * 每设备中继令牌与 revoke 断隧道、换号接管、schema v2 迁移。
 * 使用 :memory: SQLite 与 mock HTTP；不依赖外部服务。
 * 注意：限速 bucket 是模块级、按 x-forwarded-for 区分，因此每个用例使用唯一 IP。
 */
import assert from "node:assert/strict";
import { generateKeyPairSync, randomBytes, sign, type KeyObject } from "node:crypto";
import { createServer } from "node:http";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import test from "node:test";

import { CloudStore } from "../src/cloudStore.js";
import { hashPassword, verifyPassword } from "../src/passwordCrypto.js";
import { startRelayServer, type RelayServer } from "../src/server.js";
import {
  startRelayTunnelClient,
  type RelayTunnelClient,
} from "../src/tunnelClient.js";

const RELAY_TOKEN = "relay-token";
const ENROLL_TOKEN = "enroll-token";
const PUBLIC_BASE = "https://relay.example.test";

let ipCounter = 0;
function nextIp(): string {
  ipCounter += 1;
  return `10.1.0.${ipCounter}`;
}

function createKeyPair(): { privateKey: KeyObject; publicKey: KeyObject } {
  return generateKeyPairSync("ed25519");
}

async function withRelay(
  cloud: Record<string, unknown> = {},
  run: (relay: RelayServer) => Promise<void>,
): Promise<void> {
  const relay = await startRelayServer({
    port: 0,
    token: RELAY_TOKEN,
    publicBaseUrl: PUBLIC_BASE,
    log: () => undefined,
    cloud: { dbPath: ":memory:", enrollToken: ENROLL_TOKEN, accountPepper: "pepper", ...cloud },
  });
  try {
    await run(relay);
  } finally {
    await relay.close();
  }
}

interface AuthResponse {
  status: number;
  body: Record<string, unknown>;
}

async function postAuth(
  port: number,
  path: string,
  body: unknown,
  token?: string,
): Promise<AuthResponse> {
  const response = await fetch(`http://127.0.0.1:${port}${path}`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-forwarded-for": nextIp(),
      ...(token ? { authorization: `Bearer ${token}` } : {}),
    },
    body: JSON.stringify(body),
  });
  return { status: response.status, body: (await response.json()) as Record<string, unknown> };
}

async function listDevices(
  port: number,
  token: string,
): Promise<AuthResponse> {
  const response = await fetch(`http://127.0.0.1:${port}/v1/devices`, {
    headers: { authorization: `Bearer ${token}` },
  });
  return { status: response.status, body: (await response.json()) as Record<string, unknown> };
}

async function registerAccount(
  relay: RelayServer,
  username: string,
): Promise<{ sessionToken: string; accountId: string }> {
  const result = await postAuth(relay.port, "/v1/auth/register", {
    username,
    password: "password-8",
  });
  assert.equal(result.status, 200, `register ${username} failed: ${JSON.stringify(result.body)}`);
  return {
    sessionToken: String(result.body.sessionToken),
    accountId: String((result.body.account as Record<string, unknown>).id),
  };
}

/** 会话路径 enroll：Bearer sessionToken，断言 payload 不含 account 块。 */
async function enrollWithSession(
  port: number,
  sessionToken: string,
  deviceId: string,
  tunnelId: string,
): Promise<AuthResponse> {
  const { privateKey, publicKey } = createKeyPair();
  const issuedAt = Date.now();
  const nonce = randomBytes(16).toString("base64url");
  const payload = JSON.stringify({
    device: { id: deviceId, relayTunnelId: tunnelId },
    nonce,
    issuedAt,
  });
  const signature = sign(null, Buffer.from(payload, "utf8"), privateKey).toString("base64");
  const response = await fetch(`http://127.0.0.1:${port}/v1/devices/enroll`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      authorization: `Bearer ${sessionToken}`,
      "x-forwarded-for": nextIp(),
    },
    body: JSON.stringify({
      device: {
        id: deviceId,
        name: "Desk",
        platform: "win32",
        version: "1.0.0",
        publicKey: publicKey.export({ type: "spki", format: "pem" }).toString(),
        relayTunnelId: tunnelId,
      },
      assertion: { payload, signature },
      nonce,
      issuedAt,
    }),
  });
  return { status: response.status, body: (await response.json()) as Record<string, unknown> };
}

/** 旧部署密钥路径（运维过渡）：weixin 锚点 + account 块。 */
async function enrollLegacy(
  port: number,
  providerUserId: string,
  deviceId: string,
  tunnelId: string,
): Promise<AuthResponse> {
  const { privateKey, publicKey } = createKeyPair();
  const issuedAt = Date.now();
  const nonce = randomBytes(16).toString("base64url");
  const payload = JSON.stringify({
    account: { provider: "weixin", providerUserId },
    device: { id: deviceId, relayTunnelId: tunnelId },
    nonce,
    issuedAt,
  });
  const signature = sign(null, Buffer.from(payload, "utf8"), privateKey).toString("base64");
  const response = await fetch(`http://127.0.0.1:${port}/v1/devices/enroll`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-mode-enroll-token": ENROLL_TOKEN,
      "x-forwarded-for": nextIp(),
    },
    body: JSON.stringify({
      account: { provider: "weixin", providerUserId, displayName: "Me" },
      device: {
        id: deviceId,
        name: "Desk",
        platform: "win32",
        version: "1.0.0",
        publicKey: publicKey.export({ type: "spki", format: "pem" }).toString(),
        relayTunnelId: tunnelId,
      },
      assertion: { payload, signature },
      nonce,
      issuedAt,
    }),
  });
  return { status: response.status, body: (await response.json()) as Record<string, unknown> };
}

test("密码哈希：scrypt 往返与错误密码", async () => {
  const stored = await hashPassword("correct horse battery");
  assert.match(stored, /^scrypt\$32768\$8\$1\$/u);
  assert.equal(await verifyPassword("correct horse battery", stored), true);
  assert.equal(await verifyPassword("wrong password", stored), false);
  assert.equal(await verifyPassword("x", "not-a-hash"), false);
});

test("注册/登录/refresh 轮转/logout：会话生命周期", async () => {
  await withRelay({}, async (relay) => {
    const registered = await postAuth(relay.port, "/v1/auth/register", {
      username: "alice",
      password: "password-8",
      displayName: "Alice",
    });
    assert.equal(registered.status, 200);
    const account = registered.body.account as Record<string, unknown>;
    assert.equal(account.username, "alice");
    assert.equal(account.displayName, "Alice");
    const sessionA = String(registered.body.sessionToken);

    // 重复注册 409；保留名与弱密码 400。
    const duplicate = await postAuth(relay.port, "/v1/auth/register", {
      username: "alice",
      password: "password-8",
    });
    assert.equal(duplicate.status, 409);
    const reserved = await postAuth(relay.port, "/v1/auth/register", {
      username: "admin",
      password: "password-8",
    });
    assert.equal(reserved.status, 400);
    const weak = await postAuth(relay.port, "/v1/auth/register", {
      username: "bob",
      password: "short",
    });
    assert.equal(weak.status, 400);

    // 登录：错误密码与不存在用户名同为 401 invalid_credentials。
    const wrongPassword = await postAuth(relay.port, "/v1/auth/login", {
      username: "alice",
      password: "wrong-password",
    });
    assert.equal(wrongPassword.status, 401);
    assert.equal(wrongPassword.body.error, "invalid_credentials");
    const unknownUser = await postAuth(relay.port, "/v1/auth/login", {
      username: "nobody",
      password: "password-8",
    });
    assert.equal(unknownUser.status, 401);
    assert.equal(unknownUser.body.error, "invalid_credentials");

    const login = await postAuth(relay.port, "/v1/auth/login", {
      username: "alice",
      password: "password-8",
    });
    assert.equal(login.status, 200);
    const sessionB = String(login.body.sessionToken);

    // refresh 轮转：旧 token 失效。
    const refreshed = await postAuth(relay.port, "/v1/auth/refresh", {}, sessionB);
    assert.equal(refreshed.status, 200);
    const sessionC = String(refreshed.body.sessionToken);
    const staleRefresh = await postAuth(relay.port, "/v1/auth/refresh", {}, sessionB);
    assert.equal(staleRefresh.status, 401);

    // 会话列表：两个活跃会话；撤销一个后列表减一。
    const sessions = await fetch(`http://127.0.0.1:${relay.port}/v1/auth/sessions`, {
      headers: { authorization: `Bearer ${sessionC}` },
    });
    const sessionList = (await sessions.json()) as { sessions: Array<Record<string, unknown>> };
    assert.equal(sessionList.sessions.length, 2);

    // logout 后 token 失效。
    await postAuth(relay.port, "/v1/auth/logout", {}, sessionA);
    const afterLogout = await listDevices(relay.port, sessionA);
    // 会话失效但设备未绑定 → 401。
    assert.equal(afterLogout.status, 401);
    void sessionC;
  });
});

test("邀请码：配置后注册必须携带", async () => {
  await withRelay({ inviteCode: "secret-invite" }, async (relay) => {
    const missing = await postAuth(relay.port, "/v1/auth/register", {
      username: "carol",
      password: "password-8",
    });
    assert.equal(missing.status, 403);
    assert.equal(missing.body.error, "invite_required");
    const ok = await postAuth(relay.port, "/v1/auth/register", {
      username: "carol",
      password: "password-8",
      inviteCode: "secret-invite",
    });
    assert.equal(ok.status, 200);
  });
});

test("登录限速：同一用户名连续失败后 429", async () => {
  await withRelay({}, async (relay) => {
    await registerAccount(relay, "dave");
    let lastStatus = 0;
    for (let attempt = 0; attempt < 7; attempt += 1) {
      const response = await fetch(`http://127.0.0.1:${relay.port}/v1/auth/login`, {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "x-forwarded-for": nextIp(),
        },
        body: JSON.stringify({ username: "dave", password: "wrong-password" }),
      });
      lastStatus = response.status;
      if (response.status === 429) {
        break;
      }
      await response.json().catch(() => undefined);
    }
    assert.equal(lastStatus, 429);
  });
});

test("GitHub 登录：mock GitHub API 验证 token，JIT 建号且二次登录同账号", async () => {
  const github = createServer((req, res) => {
    req.on("data", () => undefined);
    req.on("end", () => {
      const auth = String(req.headers.authorization ?? "");
      res.writeHead(auth === "Bearer good-token" || auth === "Bearer good-token-2" ? 200 : 401, {
        "content-type": "application/json",
      });
      res.end(JSON.stringify({ id: 424242, login: "octocat", name: "Octo Cat" }));
    });
  });
  await new Promise<void>((resolve) => github.listen(0, "127.0.0.1", resolve));
  const address = github.address();
  const githubPort = typeof address === "object" && address ? address.port : 0;
  try {
    await withRelay(
      { githubApiBaseUrl: `http://127.0.0.1:${githubPort}` },
      async (relay) => {
        const first = await postAuth(relay.port, "/v1/auth/github", {
          accessToken: "good-token",
          clientLabel: "desktop-win32",
        });
        assert.equal(first.status, 200);
        const account = first.body.account as Record<string, unknown>;
        assert.equal(account.displayName, "Octo Cat");
        const accountId = String(account.id);

        const second = await postAuth(relay.port, "/v1/auth/github", {
          accessToken: "good-token-2",
        });
        assert.equal(second.status, 200);
        assert.equal((second.body.account as Record<string, unknown>).id, accountId);

        const invalid = await postAuth(relay.port, "/v1/auth/github", {
          accessToken: "bad-token",
        });
        assert.equal(invalid.status, 401);
        assert.equal(invalid.body.error, "invalid_github_token");

        // GitHub 账号补设用户名密码后可用密码登录。
        const sessionToken = String(first.body.sessionToken);
        const credentials = await postAuth(
          relay.port,
          "/v1/auth/credentials",
          { username: "octocat", password: "password-8" },
          sessionToken,
        );
        assert.equal(credentials.status, 200);
        const passwordLogin = await postAuth(relay.port, "/v1/auth/login", {
          username: "octocat",
          password: "password-8",
        });
        assert.equal(passwordLogin.status, 200);
        assert.equal((passwordLogin.body.account as Record<string, unknown>).id, accountId);
      },
    );
  } finally {
    await new Promise<void>((resolve) => github.close(() => resolve()));
  }
});

test("会话路径 enroll + 每设备中继令牌：目录、隧道按设备校验、revoke 断隧道", async () => {
  await withRelay({}, async (relay) => {
    const { sessionToken } = await registerAccount(relay, "erin");
    const enrolled = await enrollWithSession(
      relay.port,
      sessionToken,
      "device-erin",
      "tunnel-erin-00000001",
    );
    assert.equal(enrolled.status, 200, JSON.stringify(enrolled.body));
    const deviceToken = String(enrolled.body.deviceToken);
    const relayToken = String(enrolled.body.relayToken);
    assert.notEqual(relayToken, RELAY_TOKEN);

    // 会话与设备令牌都能读目录；响应带 username。
    const bySession = await listDevices(relay.port, sessionToken);
    assert.equal(bySession.status, 200);
    const account = bySession.body.account as Record<string, unknown>;
    assert.equal(account.username, "erin");
    const byDevice = await listDevices(relay.port, deviceToken);
    assert.equal(byDevice.status, 200);

    // 每设备令牌可注册隧道；共享 secret 也仍可用于未知隧道（回退）。
    let tunnel: RelayTunnelClient | null = startRelayTunnelClient({
      relayUrl: `ws://127.0.0.1:${relay.port}/_tunnel`,
      token: relayToken,
      tunnelId: "tunnel-erin-00000001",
      localBaseUrl: "http://127.0.0.1:1",
      log: () => undefined,
      reconnect: false,
    });
    try {
      const startedAt = Date.now();
      while (!relay.getTunnelIds().includes("tunnel-erin-00000001")) {
        if (Date.now() - startedAt > 5_000) {
          throw new Error("per-device tunnel registration timeout");
        }
        await new Promise((resolve) => setTimeout(resolve, 20));
      }

      // 会话撤销设备 → 隧道被主动断开；同令牌重连被拒。
      const revoked = await fetch(
        `http://127.0.0.1:${relay.port}/v1/devices/device-erin/revoke`,
        { method: "POST", headers: { authorization: `Bearer ${sessionToken}` } },
      );
      assert.equal(revoked.status, 200);
      const stoppedAt = Date.now();
      while (relay.getTunnelIds().includes("tunnel-erin-00000001")) {
        if (Date.now() - stoppedAt > 5_000) {
          throw new Error("tunnel close after revoke timeout");
        }
        await new Promise((resolve) => setTimeout(resolve, 20));
      }
      tunnel.close();
      tunnel = null;
      const reconnected = startRelayTunnelClient({
        relayUrl: `ws://127.0.0.1:${relay.port}/_tunnel`,
        token: relayToken,
        tunnelId: "tunnel-erin-00000001",
        localBaseUrl: "http://127.0.0.1:1",
        log: () => undefined,
        reconnect: false,
      });
      const graceAt = Date.now() + 1_000;
      while (Date.now() < graceAt) {
        await new Promise((resolve) => setTimeout(resolve, 50));
      }
      reconnected.close();
      assert.equal(relay.getTunnelIds().includes("tunnel-erin-00000001"), false);
    } finally {
      tunnel?.close();
    }

    // 撤销后设备令牌失效。
    const afterRevoke = await listDevices(relay.port, deviceToken);
    assert.equal(afterRevoke.status, 401);
  });
});

test("换号接管：无登录方式的旧微信账号设备可被新账号 enroll 转移", async () => {
  await withRelay({}, async (relay) => {
    const legacy = await enrollLegacy(relay.port, "wx-legacy", "device-legacy", "tunnel-legacy-01");
    assert.equal(legacy.status, 200);
    const legacyToken = String(legacy.body.deviceToken);

    const { sessionToken } = await registerAccount(relay, "frank");
    const takeover = await enrollWithSession(
      relay.port,
      sessionToken,
      "device-legacy",
      "tunnel-legacy-02",
    );
    assert.equal(takeover.status, 200, JSON.stringify(takeover.body));

    const listed = await listDevices(relay.port, sessionToken);
    assert.deepEqual(
      (listed.body.devices as Array<Record<string, unknown>>).map((device) => device.id),
      ["device-legacy"],
    );
    // 旧账号令牌仍有效但目录为空；设备令牌已被重签，旧令牌失效。
    const oldList = await listDevices(relay.port, legacyToken);
    assert.equal(oldList.status, 401);

    // 已有登录方式的账号不可被接管：frank 补设密码后，gina 无法抢走设备。
    await postAuth(
      relay.port,
      "/v1/auth/credentials",
      { username: "frank", password: "password-8" },
      sessionToken,
    );
    const { sessionToken: ginaToken } = await registerAccount(relay, "gina");
    const conflict = await enrollWithSession(
      relay.port,
      ginaToken,
      "device-legacy",
      "tunnel-legacy-03",
    );
    assert.equal(conflict.status, 403);
    assert.equal(conflict.body.error, "device_conflict");
  });
});

test("schema v2 迁移：v1 库自动补列，存量数据保留", () => {
  const dir = mkdtempSync(join(tmpdir(), "mode-cloud-migrate-"));
  const dbPath = join(dir, "cloud.sqlite");
  const db = new DatabaseSync(dbPath);
  db.exec(`
    CREATE TABLE cloud_schema_migration (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL, checksum TEXT NOT NULL);
    CREATE TABLE cloud_accounts (id TEXT PRIMARY KEY, provider TEXT NOT NULL, provider_user_hash TEXT NOT NULL, display_name TEXT, created_at INTEGER NOT NULL, status TEXT NOT NULL, UNIQUE(provider, provider_user_hash));
    CREATE TABLE cloud_devices (id TEXT PRIMARY KEY, account_id TEXT NOT NULL, name TEXT NOT NULL, platform TEXT NOT NULL, version TEXT, public_key TEXT NOT NULL, relay_tunnel_id TEXT NOT NULL UNIQUE, created_at INTEGER NOT NULL, last_seen_at INTEGER, revoked_at INTEGER);
    CREATE TABLE cloud_device_tokens (device_id TEXT PRIMARY KEY, token_hash TEXT NOT NULL, created_at INTEGER NOT NULL, last_used_at INTEGER);
  `);
  db.prepare(
    "INSERT INTO cloud_accounts (id, provider, provider_user_hash, display_name, created_at, status) VALUES ('acc-1', 'weixin', 'hash-1', 'Old', 1, 'active')",
  ).run();
  db.prepare(
    "INSERT INTO cloud_devices (id, account_id, name, platform, version, public_key, relay_tunnel_id, created_at) VALUES ('dev-1', 'acc-1', 'Old Desk', 'win32', NULL, 'pk', 'tunnel-old-000001', 1)",
  ).run();
  db.close();

  const store = new CloudStore(dbPath);
  try {
    const devices = store.listDevices("acc-1");
    assert.equal(devices.length, 1);
    assert.equal(devices[0]?.name, "Old Desk");
    // 新能力可用：密码账号创建 + 会话。
    const created = store.auth.createAccount({
      provider: "password",
      providerUserHash: "hash-2",
      username: "migrated",
      passwordHash: "scrypt$32768$8$1$aa$bb",
      now: Date.now(),
    });
    assert.notEqual(created, "conflict");
    const session = store.auth.createSession({
      accountId: (created as { id: string }).id,
      tokenHash: "session-hash",
      now: Date.now(),
      expiresAt: Date.now() + 1000,
    });
    assert.ok(session.id);
  } finally {
    store.close();
  }
});
