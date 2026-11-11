import assert from "node:assert/strict";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import test from "node:test";

import { startRemoteServer } from "../src/index.js";

test("remote-server 代理 GitHub Device Flow 并直接返回 Web 会话", async () => {
  const github = createServer((req, res) => {
    res.setHeader("content-type", "application/json");
    if (req.url === "/login/device/code") {
      res.end(
        JSON.stringify({
          device_code: "device-1",
          user_code: "ABCD-1234",
          verification_uri: "https://github.com/login/device",
          expires_in: 900,
          interval: 5,
        }),
      );
      return;
    }
    if (req.url === "/login/oauth/access_token") {
      res.end(JSON.stringify({ access_token: "github-token-1" }));
      return;
    }
    res.statusCode = 404;
    res.end(JSON.stringify({ error: "not_found" }));
  });
  await new Promise<void>((resolve) => github.listen(0, "127.0.0.1", resolve));
  const githubBase = `http://127.0.0.1:${(github.address() as AddressInfo).port}`;

  const server = await startRemoteServer({
    port: 0,
    host: "127.0.0.1",
    dbPath: ":memory:",
    secret: "device-flow-secret-long-enough",
    githubClientId: "client-id-1",
    githubDeviceBaseUrl: githubBase,
    githubVerifier: async (token) => {
      assert.equal(token, "github-token-1");
      return { providerUserId: "device-user", login: "device-user" };
    },
  });
  try {
    const start = await fetch(`${server.url}/api/v1/auth/github/device/start`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "{}",
    });
    const challenge = (await start.json()) as {
      deviceCode: string;
      userCode: string;
      interval: number;
    };
    assert.equal(start.status, 200);
    assert.equal(challenge.userCode, "ABCD-1234");
    assert.equal(challenge.interval, 5);

    const poll = await fetch(`${server.url}/api/v1/auth/github/device/poll`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ deviceCode: challenge.deviceCode }),
    });
    const result = (await poll.json()) as { status: string; sessionToken?: string };
    assert.equal(poll.status, 200);
    assert.equal(result.status, "ok");
    assert.ok(result.sessionToken);

    const me = await fetch(`${server.url}/api/v1/auth/me`, {
      headers: { authorization: `Bearer ${result.sessionToken}` },
    });
    assert.equal(me.status, 200);
  } finally {
    await server.close();
    await new Promise<void>((resolve) => github.close(() => resolve()));
  }
});
