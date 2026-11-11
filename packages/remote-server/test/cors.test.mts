/**
 * API CORS 白名单（docs/specs/remote-control-v3-web.md 安全节）：
 * 浏览器跨源调用 `/api/v1/*` 只在 `allowedOrigins` 命中时放行。
 */
import assert from "node:assert/strict";
import test from "node:test";

import { startRemoteServer, type RemoteServerHandle } from "../src/index.js";

async function withServer(
  allowedOrigins: string[] | undefined,
  run: (server: RemoteServerHandle) => Promise<void>,
): Promise<void> {
  const server = await startRemoteServer({
    port: 0,
    dbPath: ":memory:",
    secret: "cors-test-secret-long-enough",
    githubVerifier: async () => ({ providerUserId: "cors", login: "cors" }),
    ...(allowedOrigins ? { allowedOrigins } : {}),
  });
  try {
    await run(server);
  } finally {
    await server.close();
  }
}

test("配置白名单：预检与实际请求回显允许来源", async () => {
  await withServer(["https://web.example"], async (server) => {
    const preflight = await fetch(`${server.url}/api/v1/auth/github`, {
      method: "OPTIONS",
      headers: {
        origin: "https://web.example",
        "access-control-request-method": "POST",
        "access-control-request-headers": "authorization,content-type",
      },
    });
    assert.equal(preflight.status, 204);
    assert.equal(preflight.headers.get("access-control-allow-origin"), "https://web.example");
    assert.match(preflight.headers.get("access-control-allow-methods") ?? "", /POST/u);
    assert.match(
      preflight.headers.get("access-control-allow-headers") ?? "",
      /authorization/u,
    );

    const actual = await fetch(`${server.url}/api/v1/auth/github`, {
      method: "POST",
      headers: {
        origin: "https://web.example",
        "content-type": "application/json",
      },
      body: JSON.stringify({ accessToken: "cors-token" }),
    });
    assert.equal(actual.status, 200);
    assert.equal(actual.headers.get("access-control-allow-origin"), "https://web.example");
  });
});

test("未命中的来源不回显 ACAO（浏览器会拦截读取）", async () => {
  await withServer(["https://web.example"], async (server) => {
    const preflight = await fetch(`${server.url}/api/v1/auth/github`, {
      method: "OPTIONS",
      headers: { origin: "https://evil.example", "access-control-request-method": "POST" },
    });
    assert.equal(preflight.status, 204);
    assert.equal(preflight.headers.get("access-control-allow-origin"), null);

    const actual = await fetch(`${server.url}/api/v1/auth/github`, {
      method: "POST",
      headers: { origin: "https://evil.example", "content-type": "application/json" },
      body: JSON.stringify({ accessToken: "cors-token" }),
    });
    assert.equal(actual.status, 200);
    assert.equal(actual.headers.get("access-control-allow-origin"), null);
  });
});

test("未配置白名单：保持同源行为，不发 CORS 头", async () => {
  await withServer(undefined, async (server) => {
    const response = await fetch(`${server.url}/api/v1/auth/github`, {
      method: "POST",
      headers: { origin: "https://web.example", "content-type": "application/json" },
      body: JSON.stringify({ accessToken: "cors-token" }),
    });
    assert.equal(response.status, 200);
    assert.equal(response.headers.get("access-control-allow-origin"), null);
  });
});
