/**
 * Web 主控的远程环境探测与连接发起边界
 * （docs/specs/web-remote-workspace-ssh-docker.md §5/§7）。
 *
 * 不访问真实 SSH/Docker：只验证鉴权、空结果收口、schema 校验与「缺资产源」前置拒绝。
 */
import assert from "node:assert/strict";
import type { AddressInfo } from "node:net";
import test from "node:test";

import type { ServiceCollection } from "@mode/services";
import { createHttpServer } from "../src/http.js";

const TOKEN = "remote-env-test-token";

async function startServer(): Promise<{
  base: string;
  close: () => Promise<void>;
}> {
  const server = createHttpServer({} as unknown as ServiceCollection, 0, {
    authToken: TOKEN,
    authRequired: true,
  });
  await new Promise<void>((resolve) => server.once("listening", () => resolve()));
  const port = (server.address() as AddressInfo).port;
  return {
    base: `http://127.0.0.1:${String(port)}`,
    close: () => new Promise<void>((resolve) => server.close(() => resolve())),
  };
}

function withoutRemoteAssetEnv<T>(run: () => Promise<T>): Promise<T> {
  const keys = [
    "MODE_REMOTE_ASSET_CDN_BASE_URL",
    "ZCODIUM_REMOTE_ASSET_CDN_BASE_URL",
    "ZCODE_REMOTE_ASSET_CDN_BASE_URL",
    "MODE_MOCK_CDN_DIR",
  ];
  const saved = new Map(keys.map((key) => [key, process.env[key]]));
  for (const key of keys) {
    delete process.env[key];
  }
  return run().finally(() => {
    for (const [key, value] of saved) {
      if (value === undefined) {
        delete process.env[key];
      } else {
        process.env[key] = value;
      }
    }
  });
}

test("远程环境端点需要令牌，探测失败按空结果返回", async () => {
  const { base, close } = await startServer();
  try {
    const unauthorized = await fetch(`${base}/api/remote-environments/ssh-aliases`);
    assert.equal(unauthorized.status, 401);

    const aliases = await fetch(`${base}/api/remote-environments/ssh-aliases?token=${TOKEN}`);
    assert.equal(aliases.status, 200);
    assert.ok(Array.isArray(((await aliases.json()) as { aliases?: unknown }).aliases));

    const docker = await fetch(`${base}/api/remote-environments/docker?token=${TOKEN}`);
    assert.equal(docker.status, 200);
    const payload = (await docker.json()) as { available?: unknown; containers?: unknown };
    assert.equal(typeof payload.available, "boolean");
    assert.ok(Array.isArray(payload.containers));
  } finally {
    await close();
  }
});

test("connect-remote：schema 校验、LAN 目标拒绝、缺资产源前置拒绝", async () => {
  const { base, close } = await startServer();
  try {
    const invalid = await fetch(`${base}/api/connect-remote?token=${TOKEN}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ kind: "ssh" }),
    });
    assert.equal(invalid.status, 400);

    const lan = await fetch(`${base}/api/connect-remote?token=${TOKEN}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ kind: "lan", host: "127.0.0.1", port: 3030 }),
    });
    assert.equal(lan.status, 400);

    await withoutRemoteAssetEnv(async () => {
      const noAssets = await fetch(`${base}/api/connect-remote?token=${TOKEN}`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ kind: "ssh", host: "127.0.0.1", username: "demo" }),
      });
      assert.equal(noAssets.status, 409);
      assert.equal(
        ((await noAssets.json()) as { error?: unknown }).error,
        "remote_assets_unavailable",
      );
    });
  } finally {
    await close();
  }
});
