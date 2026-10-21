/**
 * Mode Cloud 控制面：enroll、设备目录、在线状态、账号隔离、撤销。
 * 使用 :memory: SQLite；不依赖外部服务。
 */
import assert from "node:assert/strict";
import { generateKeyPairSync, randomBytes, sign } from "node:crypto";
import { createServer } from "node:http";
import test from "node:test";

import type { KeyObject } from "node:crypto";

import { startRelayServer, type RelayServer } from "../src/server.js";
import {
  startRelayTunnelClient,
  type RelayTunnelClient,
} from "../src/tunnelClient.js";

const RELAY_TOKEN = "relay-token";
const ENROLL_TOKEN = "enroll-token";
const PUBLIC_BASE = "https://relay.example.test";

interface EnrollOptions {
  port: number;
  providerUserId: string;
  deviceId: string;
  tunnelId: string;
  privateKey: KeyObject;
  publicKey: KeyObject;
  enrollToken?: string;
  ipSeed: number;
}

let ipCounter = 0;

async function enroll(options: EnrollOptions): Promise<{
  status: number;
  body: Record<string, unknown>;
}> {
  ipCounter += 1;
  const issuedAt = Date.now();
  const nonce = randomBytes(16).toString("base64url");
  const payload = JSON.stringify({
    account: { provider: "weixin", providerUserId: options.providerUserId },
    device: { id: options.deviceId, relayTunnelId: options.tunnelId },
    nonce,
    issuedAt,
  });
  const signature = sign(null, Buffer.from(payload, "utf8"), options.privateKey).toString(
    "base64",
  );
  const response = await fetch(`http://127.0.0.1:${options.port}/v1/devices/enroll`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-mode-enroll-token": options.enrollToken ?? ENROLL_TOKEN,
      "x-forwarded-for": `10.0.0.${options.ipSeed ?? ipCounter}`,
    },
    body: JSON.stringify({
      account: {
        provider: "weixin",
        providerUserId: options.providerUserId,
        displayName: "Me",
      },
      device: {
        id: options.deviceId,
        name: "Desk",
        platform: "win32",
        version: "1.0.0",
        publicKey: options.publicKey.export({ type: "spki", format: "pem" }).toString(),
        relayTunnelId: options.tunnelId,
      },
      assertion: { payload, signature },
      nonce,
      issuedAt,
    }),
  });
  return { status: response.status, body: (await response.json()) as Record<string, unknown> };
}

async function listDevices(
  port: number,
  token: string,
): Promise<{ status: number; body: Record<string, unknown> }> {
  const response = await fetch(`http://127.0.0.1:${port}/v1/devices`, {
    headers: { authorization: `Bearer ${token}` },
  });
  return { status: response.status, body: (await response.json()) as Record<string, unknown> };
}

function createKeyPair(): { privateKey: KeyObject; publicKey: KeyObject } {
  const { privateKey, publicKey } = generateKeyPairSync("ed25519");
  return { privateKey, publicKey };
}

async function withRelay(
  run: (relay: RelayServer) => Promise<void>,
): Promise<void> {
  const relay = await startRelayServer({
    port: 0,
    token: RELAY_TOKEN,
    publicBaseUrl: PUBLIC_BASE,
    log: () => undefined,
    cloud: { dbPath: ":memory:", enrollToken: ENROLL_TOKEN, accountPepper: "pepper" },
  });
  try {
    await run(relay);
  } finally {
    await relay.close();
  }
}

test("enroll + 目录：账号、设备、公网地址与离线状态", async () => {
  await withRelay(async (relay) => {
    const keys = createKeyPair();
    const enrolled = await enroll({
      port: relay.port,
      providerUserId: "wx-user-1",
      deviceId: "device-1",
      tunnelId: "tunnel-0000000001",
      ...keys,
      ipSeed: 11,
    });
    assert.equal(enrolled.status, 200);
    const deviceToken = enrolled.body.deviceToken;
    assert.equal(typeof deviceToken, "string");

    const listed = await listDevices(relay.port, String(deviceToken));
    assert.equal(listed.status, 200);
    const devices = listed.body.devices as Array<Record<string, unknown>>;
    assert.equal(devices.length, 1);
    assert.equal(devices[0]?.id, "device-1");
    assert.equal(devices[0]?.online, false);
    assert.equal(devices[0]?.relayUrl, `${PUBLIC_BASE}/d/tunnel-0000000001`);
  });
});

test("在线状态跟随隧道注册与断开", async () => {
  await withRelay(async (relay) => {
    const keys = createKeyPair();
    const enrolled = await enroll({
      port: relay.port,
      providerUserId: "wx-user-2",
      deviceId: "device-2",
      tunnelId: "tunnel-0000000002",
      ...keys,
      ipSeed: 12,
    });
    const deviceToken = String(enrolled.body.deviceToken);

    let tunnel: RelayTunnelClient | null = startRelayTunnelClient({
      relayUrl: `ws://127.0.0.1:${relay.port}/_tunnel`,
      token: RELAY_TOKEN,
      tunnelId: "tunnel-0000000002",
      localBaseUrl: "http://127.0.0.1:1",
      log: () => undefined,
      reconnect: false,
    });
    try {
      const startedAt = Date.now();
      while (!relay.getTunnelIds().includes("tunnel-0000000002")) {
        if (Date.now() - startedAt > 5_000) {
          throw new Error("tunnel registration timeout");
        }
        await new Promise((resolve) => setTimeout(resolve, 20));
      }
      const online = await listDevices(relay.port, deviceToken);
      assert.equal((online.body.devices as Array<Record<string, unknown>>)[0]?.online, true);

      tunnel.close();
      tunnel = null;
      const stoppedAt = Date.now();
      while (relay.getTunnelIds().includes("tunnel-0000000002")) {
        if (Date.now() - stoppedAt > 5_000) {
          throw new Error("tunnel disconnect timeout");
        }
        await new Promise((resolve) => setTimeout(resolve, 20));
      }
      const offline = await listDevices(relay.port, deviceToken);
      assert.equal((offline.body.devices as Array<Record<string, unknown>>)[0]?.online, false);
    } finally {
      tunnel?.close();
    }
  });
});

test("鉴权与账号隔离：错误 enrollment token / 无 Bearer / 不同账号互不可见", async () => {
  await withRelay(async (relay) => {
    const keysA = createKeyPair();
    const keysB = createKeyPair();
    const wrong = await enroll({
      port: relay.port,
      providerUserId: "wx-a",
      deviceId: "device-a",
      tunnelId: "tunnel-000000000a",
      enrollToken: "wrong-token",
      ...keysA,
      ipSeed: 21,
    });
    assert.equal(wrong.status, 401);

    const accountA = await enroll({
      port: relay.port,
      providerUserId: "wx-a",
      deviceId: "device-a",
      tunnelId: "tunnel-000000000a",
      ...keysA,
      ipSeed: 22,
    });
    const accountB = await enroll({
      port: relay.port,
      providerUserId: "wx-b",
      deviceId: "device-b",
      tunnelId: "tunnel-000000000b",
      ...keysB,
      ipSeed: 23,
    });
    assert.equal(accountA.status, 200);
    assert.equal(accountB.status, 200);

    const noToken = await fetch(`http://127.0.0.1:${relay.port}/v1/devices`);
    assert.equal(noToken.status, 401);

    const listA = await listDevices(relay.port, String(accountA.body.deviceToken));
    const listB = await listDevices(relay.port, String(accountB.body.deviceToken));
    assert.deepEqual(
      (listA.body.devices as Array<Record<string, unknown>>).map((device) => device.id),
      ["device-a"],
    );
    assert.deepEqual(
      (listB.body.devices as Array<Record<string, unknown>>).map((device) => device.id),
      ["device-b"],
    );
  });
});

test("撤销设备：目录立即移除，令牌失效", async () => {
  await withRelay(async (relay) => {
    const keys = createKeyPair();
    const enrolled = await enroll({
      port: relay.port,
      providerUserId: "wx-revoke",
      deviceId: "device-revoke",
      tunnelId: "tunnel-000000000c",
      ...keys,
      ipSeed: 31,
    });
    const deviceToken = String(enrolled.body.deviceToken);

    const revoked = await fetch(
      `http://127.0.0.1:${relay.port}/v1/devices/device-revoke/revoke`,
      { method: "POST", headers: { authorization: `Bearer ${deviceToken}` } },
    );
    assert.equal(revoked.status, 200);
    const after = await listDevices(relay.port, deviceToken);
    assert.equal(after.status, 401);
  });
});

test("云端 Bot 登录：微信确认 6 位码后签发设备与中继令牌", async () => {
  let deliverCode = "";
  const mockIlink = createServer((req, res) => {
    req.on("data", () => undefined);
    req.on("end", () => {
      const payload = deliverCode
        ? {
            ret: 0,
            errcode: 0,
            msgs: [{ from_user_id: "wx-login-user", text: `login ${deliverCode}` }],
            get_updates_buf: "cursor-1",
          }
        : { ret: 0, errcode: 0, msgs: [], get_updates_buf: "cursor-1" };
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify(payload));
    });
  });
  await new Promise<void>((resolve) => mockIlink.listen(0, "127.0.0.1", resolve));
  const address = mockIlink.address();
  const mockPort = typeof address === "object" && address ? address.port : 0;

  const relay = await startRelayServer({
    port: 0,
    token: RELAY_TOKEN,
    publicBaseUrl: PUBLIC_BASE,
    log: () => undefined,
    cloud: {
      dbPath: ":memory:",
      enrollToken: ENROLL_TOKEN,
      accountPepper: "pepper",
      wechatBaseUrl: `http://127.0.0.1:${mockPort}`,
    },
  });
  try {
    const uploaded = await fetch(`http://127.0.0.1:${relay.port}/v1/admin/wechat-bot`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-mode-enroll-token": ENROLL_TOKEN,
      },
      body: JSON.stringify({ token: "wechat-token", allowedUser: "wx-login-user" }),
    });
    assert.equal(uploaded.status, 200);
    const health = (await (
      await fetch(`http://127.0.0.1:${relay.port}/v1/health`)
    ).json()) as { wechatLogin?: boolean };
    assert.equal(health.wechatLogin, true);

    const started = await fetch(`http://127.0.0.1:${relay.port}/v1/auth/start`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "{}",
    });
    assert.equal(started.status, 200);
    const challenge = (await started.json()) as { code?: string };
    const code = String(challenge.code ?? "");
    assert.match(code, /^\d{6}$/u);

    const keys = createKeyPair();
    const device = {
      id: "device-login-1",
      name: "Login Desk",
      platform: "win32",
      version: "1.0.0",
      publicKey: keys.publicKey.export({ type: "spki", format: "pem" }).toString(),
      relayTunnelId: "tunnel-000000000d",
    };
    const poll = async (): Promise<{ status: number; body: Record<string, unknown> }> => {
      const response = await fetch(`http://127.0.0.1:${relay.port}/v1/auth/poll`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ code, device }),
      });
      return { status: response.status, body: (await response.json()) as Record<string, unknown> };
    };

    const beforeApproval = await poll();
    assert.equal(beforeApproval.status, 202);

    deliverCode = code;
    let approved: Record<string, unknown> | null = null;
    const deadline = Date.now() + 6_000;
    while (Date.now() < deadline) {
      const result = await poll();
      if (result.status === 200) {
        approved = result.body;
        break;
      }
      await new Promise((resolve) => setTimeout(resolve, 200));
    }
    assert.ok(approved, "云端 Bot 应在收到微信码后批准登录");
    assert.equal(approved.relayToken, RELAY_TOKEN);
    assert.equal(typeof approved.deviceToken, "string");

    const listed = await listDevices(relay.port, String(approved.deviceToken));
    assert.equal(listed.status, 200);
    assert.equal(
      (listed.body.devices as Array<Record<string, unknown>>)[0]?.id,
      "device-login-1",
    );
  } finally {
    await relay.close();
    await new Promise<void>((resolve) => mockIlink.close(() => resolve()));
  }
});

test("健康检查：控制面启用且数据库可读", async () => {
  await withRelay(async (relay) => {
    const response = await fetch(`http://127.0.0.1:${relay.port}/v1/health`);
    assert.equal(response.status, 200);
    const body = (await response.json()) as Record<string, unknown>;
    assert.equal(body.ok, true);
    assert.equal(body.control, true);
  });
});
