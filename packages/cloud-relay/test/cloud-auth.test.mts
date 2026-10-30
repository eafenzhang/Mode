/**
 * Mode Cloud 个人账号面（docs/specs/cloud-personal-accounts.md，v2）：
 * 仅 GitHub 登录（mock GitHub API）、会话轮转、register/login/credentials 下线、
 * 会话路径 enroll（访问令牌加密入目录）、设备冲突、schema v3 迁移。
 * 使用 :memory: SQLite 与 mock HTTP；不依赖外部服务。
 */
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import test from "node:test";

import { startRelayServer } from "../src/server.js";
import { CloudStore } from "../src/cloudStore.js";
import {
  DB_KEY,
  enrollWithSession,
  githubSession,
  listDevices,
  postJson,
  startMockGithub,
  withRelay,
} from "./helpers.mts";

test("密码注册/登录/补设已下线：全部 404", async () => {
  await withRelay({}, async (relay) => {
    for (const path of ["/v1/auth/register", "/v1/auth/login", "/v1/auth/credentials"]) {
      const result = await postJson(relay.port, path, { username: "a", password: "b" });
      assert.equal(result.status, 404, path);
    }
  });
});

test("GitHub 登录：JIT 建号、二次登录同账号、无效 token 401、会话轮转与撤销", async () => {
  const github = await startMockGithub({
    tokens: {
      "good-token": { id: 424242, login: "octocat", name: "Octo Cat" },
      "good-token-2": { id: 424242, login: "octocat", name: "Octo Cat" },
    },
  });
  try {
    await withRelay({ githubApiBaseUrl: `http://127.0.0.1:${github.port}` }, async (relay) => {
      const first = await githubSession(relay, "good-token");
      assert.equal(first.displayName, "Octo Cat");

      const second = await githubSession(relay, "good-token-2");
      assert.equal(second.accountId, first.accountId, "同一 GitHub 用户必须复用同一账号");

      const invalid = await postJson(relay.port, "/v1/auth/github", { accessToken: "bad" });
      assert.equal(invalid.status, 401);
      assert.equal(invalid.body.error, "invalid_github_token");

      // refresh 轮转：旧 token 立即失效。
      const refreshed = await postJson(relay.port, "/v1/auth/refresh", {}, first.sessionToken);
      assert.equal(refreshed.status, 200);
      const stale = await postJson(relay.port, "/v1/auth/refresh", {}, first.sessionToken);
      assert.equal(stale.status, 401);

      // 会话列表 + 撤销。
      const sessions = await fetch(`http://127.0.0.1:${relay.port}/v1/auth/sessions`, {
        headers: { authorization: `Bearer ${String(refreshed.body.sessionToken)}` },
      });
      const list = (await sessions.json()) as { sessions: Array<Record<string, unknown>> };
      // 两次登录 + refresh 轮转：第二次登录的会话仍活跃。
      assert.equal(list.sessions.length, 2);

      const logout = await postJson(
        relay.port,
        "/v1/auth/logout",
        {},
        String(refreshed.body.sessionToken),
      );
      assert.equal(logout.status, 200);
      const afterLogout = await listDevices(relay.port, String(refreshed.body.sessionToken));
      assert.equal(afterLogout.status, 401);
    });
  } finally {
    await github.close();
  }
});

test("会话路径 enroll：访问令牌加密入目录，目录下发明文令牌与地址", async () => {
  const github = await startMockGithub({ tokens: { t1: { id: 1, login: "erin" } } });
  try {
    await withRelay(
      {
        githubApiBaseUrl: `http://127.0.0.1:${github.port}`,
        // 把库放到临时目录以便直接读取密文列。
      },
      async (relay) => {
        const { sessionToken } = await githubSession(relay, "t1");
        const enrolled = await enrollWithSession(relay.port, sessionToken, {
          deviceId: "device-erin",
          tunnelId: "tunnel-erin-00000001",
          accessToken: "agent-secret-token-123456",
          addresses: ["192.168.31.20:47821", "100.100.1.5:47821"],
        });
        assert.equal(enrolled.status, 200, JSON.stringify(enrolled.body));
        const deviceToken = String(enrolled.body.deviceToken);
        assert.notEqual(String(enrolled.body.relayToken), "relay-token");

        // 目录：会话与设备令牌都能读；accessToken 明文仅同账号可见。
        const bySession = await listDevices(relay.port, sessionToken);
        assert.equal(bySession.status, 200);
        const account = bySession.body.account as Record<string, unknown>;
        assert.equal(account.displayName, "erin");
        assert.equal(account.username, undefined);
        const devices = bySession.body.devices as Array<Record<string, unknown>>;
        assert.equal(devices.length, 1);
        assert.equal(devices[0]?.accessToken, "agent-secret-token-123456");
        assert.deepEqual(devices[0]?.addresses, [
          "192.168.31.20:47821",
          "100.100.1.5:47821",
        ]);
        const byDevice = await listDevices(relay.port, deviceToken);
        assert.equal(byDevice.status, 200);
      },
    );
  } finally {
    await github.close();
  }
});

test("设备属于其他账号时 device_conflict；接管已删除", async () => {
  const github = await startMockGithub({
    tokens: { ta: { id: 10, login: "alice" }, tb: { id: 20, login: "bob" } },
  });
  try {
    await withRelay({ githubApiBaseUrl: `http://127.0.0.1:${github.port}` }, async (relay) => {
      const alice = await githubSession(relay, "ta");
      const bob = await githubSession(relay, "tb");
      const enrolled = await enrollWithSession(relay.port, alice.sessionToken, {
        deviceId: "device-shared",
        tunnelId: "tunnel-shared-000001",
      });
      assert.equal(enrolled.status, 200);
      const conflict = await enrollWithSession(relay.port, bob.sessionToken, {
        deviceId: "device-shared",
        tunnelId: "tunnel-shared-000002",
      });
      assert.equal(conflict.status, 403);
      assert.equal(conflict.body.error, "device_conflict");
    });
  } finally {
    await github.close();
  }
});

test("schema v3 迁移：v2 库补列删密码列，存量设备保留", () => {
  const dir = mkdtempSync(join(tmpdir(), "mode-cloud-migrate-"));
  const dbPath = join(dir, "cloud.sqlite");
  const db = new DatabaseSync(dbPath);
  db.exec(`
    CREATE TABLE cloud_schema_migration (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL, checksum TEXT NOT NULL);
    CREATE TABLE cloud_accounts (id TEXT PRIMARY KEY, provider TEXT NOT NULL, provider_user_hash TEXT NOT NULL, display_name TEXT, created_at INTEGER NOT NULL, status TEXT NOT NULL, username TEXT, password_hash TEXT, updated_at INTEGER, UNIQUE(provider, provider_user_hash));
    CREATE TABLE cloud_devices (id TEXT PRIMARY KEY, account_id TEXT NOT NULL, name TEXT NOT NULL, platform TEXT NOT NULL, version TEXT, public_key TEXT NOT NULL, relay_tunnel_id TEXT NOT NULL UNIQUE, created_at INTEGER NOT NULL, last_seen_at INTEGER, revoked_at INTEGER, relay_token_hash TEXT);
    CREATE TABLE cloud_device_tokens (device_id TEXT PRIMARY KEY, token_hash TEXT NOT NULL, created_at INTEGER NOT NULL, last_used_at INTEGER);
    CREATE TABLE cloud_user_sessions (id TEXT PRIMARY KEY, account_id TEXT NOT NULL, token_hash TEXT NOT NULL UNIQUE, client_label TEXT, created_at INTEGER NOT NULL, last_used_at INTEGER, expires_at INTEGER NOT NULL, revoked_at INTEGER);
  `);
  db.prepare(
    "INSERT INTO cloud_accounts (id, provider, provider_user_hash, display_name, created_at, status, username, password_hash) VALUES ('acc-1', 'github', 'hash-1', 'Old', 1, 'active', 'old', 'scrypt$x$y$z')",
  ).run();
  db.prepare(
    "INSERT INTO cloud_devices (id, account_id, name, platform, version, public_key, relay_tunnel_id, created_at, relay_token_hash) VALUES ('dev-1', 'acc-1', 'Old Desk', 'win32', NULL, 'pk', 'tunnel-old-000001', 1, 'rtx')",
  ).run();
  db.close();

  const store = new CloudStore(dbPath);
  try {
    const devices = store.listDevices("acc-1");
    assert.equal(devices.length, 1);
    assert.equal(devices[0]?.name, "Old Desk");
    // GitHub-only：密码列已删除；GitHub JIT 建号可用。
    const created = store.auth.createAccount({
      providerUserHash: "hash-2",
      displayName: "New",
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

test("访问令牌密文落库且密钥无关字符串不可见；密钥错误时目录降级", async () => {
  const dir = mkdtempSync(join(tmpdir(), "mode-cloud-cipher-"));
  const dbPath = join(dir, "cloud.sqlite");
  const github = await startMockGithub({ tokens: { t1: { id: 7, login: "cleo" } } });
  try {
    const relay = await startRelayServer({
      port: 0,
      token: "relay-token",
      publicBaseUrl: "https://relay.example.test",
      log: () => undefined,
      cloud: {
        dbPath,
        accountPepper: "pepper",
        dbKey: DB_KEY,
        githubApiBaseUrl: `http://127.0.0.1:${github.port}`,
      },
    });
    const { sessionToken } = await githubSession(relay, "t1");
    const enrolled = await enrollWithSession(relay.port, sessionToken, {
      deviceId: "device-cipher",
      tunnelId: "tunnel-cipher-000001",
      accessToken: "super-secret-agent-token",
    });
    assert.equal(enrolled.status, 200);
    await relay.close();

    // 密文落库：原始 SQLite（含 WAL）不含明文令牌。
    let raw = "";
    for (const suffix of ["", "-wal", "-shm"]) {
      try {
        raw += readFileSync(dbPath + suffix, "utf8");
      } catch {
        // 文件可能不存在。
      }
    }
    assert.equal(raw.includes("super-secret-agent-token"), false);

    // 换一把错误密钥打开：目录降级为 tokenAvailable:false，不崩溃。
    const broken = await startRelayServer({
      port: 0,
      token: "relay-token",
      publicBaseUrl: "https://relay.example.test",
      log: () => undefined,
      cloud: { dbPath, accountPepper: "pepper", dbKey: "wrong-key", githubApiBaseUrl: `http://127.0.0.1:${github.port}` },
    });
    try {
      const session = await postJson(broken.port, "/v1/auth/github", {
        accessToken: "t1",
      });
      assert.equal(session.status, 200);
      const listed = await listDevices(broken.port, String(session.body.sessionToken));
      assert.equal(listed.status, 200);
      const devices = listed.body.devices as Array<Record<string, unknown>>;
      assert.equal(devices[0]?.accessToken, undefined);
      assert.equal(devices[0]?.tokenAvailable, false);
    } finally {
      await broken.close();
    }
  } finally {
    await github.close();
  }
});

test("升级认领：非 GitHub 旧身份的遗留设备可被首个 GitHub 账号 enroll 转移", async () => {
  const dir = mkdtempSync(join(tmpdir(), "mode-cloud-claim-"));
  const dbPath = join(dir, "cloud.sqlite");
  const github = await startMockGithub({ tokens: { t1: { id: 99, login: "claimer" } } });
  try {
    // 直接用旧 store 播种一个 weixin 锚点账号 + 设备（模拟 v1 升级前的存量数据）。
    const seeded = new CloudStore(dbPath);
    const seededResult = seeded.enroll({
      provider: "weixin",
      providerUserHash: "legacy-wx-hash",
      displayName: "Legacy",
      device: {
        id: "device-legacy",
        name: "Old Desk",
        platform: "win32",
        publicKey: "-----BEGIN PUBLIC KEY-----x-----END PUBLIC KEY-----",
        relayTunnelId: "tunnel-legacy-000001",
        accessTokenCipher: "cipher",
      },
      tokenHash: "legacy-device-token-hash",
      relayTokenHash: "legacy-relay-hash",
      now: Date.now(),
    });
    assert.equal(seededResult.ok, true);
    seeded.close();

    const relay = await startRelayServer({
      port: 0,
      token: "relay-token",
      publicBaseUrl: "https://relay.example.test",
      log: () => undefined,
      cloud: {
        dbPath,
        accountPepper: "pepper",
        dbKey: DB_KEY,
        githubApiBaseUrl: `http://127.0.0.1:${github.port}`,
      },
    });
    try {
      const { sessionToken } = await githubSession(relay, "t1");
      const claimed = await enrollWithSession(relay.port, sessionToken, {
        deviceId: "device-legacy",
        tunnelId: "tunnel-legacy-000002",
      });
      assert.equal(claimed.status, 200, JSON.stringify(claimed.body));
      const listed = await listDevices(relay.port, sessionToken);
      assert.deepEqual(
        (listed.body.devices as Array<Record<string, unknown>>).map((device) => device.id),
        ["device-legacy"],
      );
    } finally {
      await relay.close();
    }
  } finally {
    await github.close();
  }
});
