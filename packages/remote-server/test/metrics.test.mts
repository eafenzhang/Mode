import assert from "node:assert/strict";
import { request } from "node:http";
import test from "node:test";

import { startRemoteServer } from "../src/index.js";

function get(url: string, headers: Record<string, string> = {}): Promise<{
  status: number;
  body: string;
}> {
  return new Promise((resolve, reject) => {
    const req = request(url, { headers }, (res) => {
      let body = "";
      res.setEncoding("utf8");
      res.on("data", (chunk) => {
        body += chunk;
      });
      res.on("end", () => resolve({ status: res.statusCode ?? 0, body }));
    });
    req.once("error", reject);
    req.end();
  });
}

test("未配置 metrics token 时不暴露 /metrics", async () => {
  const server = await startRemoteServer({
    port: 0,
    host: "127.0.0.1",
    dbPath: ":memory:",
    secret: "metrics-test-secret-long-enough",
    githubVerifier: async () => ({ providerUserId: "m1", login: "m1" }),
  });
  try {
    const response = await get(`${server.url}/metrics`);
    assert.equal(response.status, 404);
  } finally {
    await server.close();
  }
});

test("配置 metrics token 后 Bearer 可读取 Prometheus 计数", async () => {
  const server = await startRemoteServer({
    port: 0,
    host: "127.0.0.1",
    dbPath: ":memory:",
    secret: "metrics-test-secret-long-enough",
    metricsToken: "metrics-token-1",
    githubVerifier: async () => ({ providerUserId: "m2", login: "m2" }),
  });
  try {
    const unauthorized = await get(`${server.url}/metrics`);
    assert.equal(unauthorized.status, 401);

    const response = await get(`${server.url}/metrics`, {
      authorization: "Bearer metrics-token-1",
    });
    assert.equal(response.status, 200);
    assert.match(response.body, /mode_remote_uptime_seconds \d+/);
    assert.match(response.body, /mode_remote_control_connections 0/);
    assert.match(response.body, /mode_remote_relay_bytes_total 0/);

    const ready = await get(`${server.url}/ready`);
    assert.equal(ready.status, 200);
    assert.equal(ready.body, "ready");
  } finally {
    await server.close();
  }
});
