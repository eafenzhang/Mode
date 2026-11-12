import assert from "node:assert/strict";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import test from "node:test";

import { startRemoteServer } from "../src/index.js";

async function startFakeGithub(): Promise<{
  base: string;
  close: () => Promise<void>;
  lastBody: () => string;
}> {
  let lastBody = "";
  const github = createServer((req, res) => {
    res.setHeader("content-type", "application/json");
    if (req.url === "/login/oauth/access_token") {
      let raw = "";
      req.on("data", (chunk) => {
        raw += chunk;
      });
      req.on("end", () => {
        lastBody = raw;
        res.end(JSON.stringify({ access_token: "oauth-github-token" }));
      });
      return;
    }
    res.statusCode = 404;
    res.end(JSON.stringify({ error: "not_found" }));
  });
  await new Promise<void>((resolve) => github.listen(0, "127.0.0.1", resolve));
  const base = `http://127.0.0.1:${(github.address() as AddressInfo).port}`;
  return {
    base,
    lastBody: () => lastBody,
    close: () => new Promise<void>((resolve) => github.close(() => resolve())),
  };
}

test("GitHub OAuth 回调：start → callback 换 token → poll 返回会话", async () => {
  const github = await startFakeGithub();
  const server = await startRemoteServer({
    port: 0,
    host: "127.0.0.1",
    dbPath: ":memory:",
    secret: "oauth-flow-secret-long-enough",
    githubClientId: "client-id-1",
    githubClientSecret: "client-secret-1",
    githubOAuthCallbackUrl: "https://remote.example.test/api/v1/auth/github/oauth/callback",
    githubOAuthAuthorizeBaseUrl: github.base,
    githubOAuthTokenBaseUrl: github.base,
    allowedOrigins: ["http://localhost:5199"],
    githubVerifier: async (token) => {
      assert.equal(token, "oauth-github-token");
      return { providerUserId: "oauth-user", login: "oauth-user" };
    },
  });
  try {
    const start = await fetch(`${server.url}/api/v1/auth/github/oauth/start`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ returnUrl: "http://localhost:5199/v3" }),
    });
    const challenge = (await start.json()) as {
      authorizationUrl: string;
      state: string;
      pollToken: string;
      interval: number;
    };
    assert.equal(start.status, 200, JSON.stringify(challenge));
    const authorize = new URL(challenge.authorizationUrl);
    assert.equal(
      `${authorize.origin}${authorize.pathname}`,
      `${github.base}/login/oauth/authorize`,
    );
    assert.equal(authorize.searchParams.get("client_id"), "client-id-1");
    assert.equal(
      authorize.searchParams.get("redirect_uri"),
      "https://remote.example.test/api/v1/auth/github/oauth/callback",
    );
    assert.equal(authorize.searchParams.get("scope"), "read:user");
    assert.equal(authorize.searchParams.get("state"), challenge.state);

    const pending = await fetch(`${server.url}/api/v1/auth/github/oauth/poll`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ state: challenge.state, pollToken: challenge.pollToken }),
    });
    assert.equal(((await pending.json()) as { status: string }).status, "pending");

    const callback = await fetch(
      `${server.url}/api/v1/auth/github/oauth/callback?code=code-1&state=${encodeURIComponent(challenge.state)}`,
      { redirect: "manual" },
    );
    assert.equal(callback.status, 303);
    assert.match(callback.headers.get("location") ?? "", /^http:\/\/localhost:5199\/v3\?oauth=1$/u);
    const exchange = new URLSearchParams(github.lastBody());
    assert.equal(exchange.get("client_secret"), "client-secret-1");
    assert.equal(exchange.get("code"), "code-1");
    assert.equal(
      exchange.get("redirect_uri"),
      "https://remote.example.test/api/v1/auth/github/oauth/callback",
    );

    const poll = await fetch(`${server.url}/api/v1/auth/github/oauth/poll`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ state: challenge.state, pollToken: challenge.pollToken }),
    });
    const result = (await poll.json()) as { status: string; sessionToken?: string };
    assert.equal(result.status, "ok");
    assert.ok(result.sessionToken);
    const me = await fetch(`${server.url}/api/v1/auth/me`, {
      headers: { authorization: `Bearer ${result.sessionToken}` },
    });
    assert.equal(me.status, 200);

    const replay = await fetch(
      `${server.url}/api/v1/auth/github/oauth/callback?code=code-1&state=${encodeURIComponent(challenge.state)}`,
    );
    assert.equal(replay.status, 409);
  } finally {
    await server.close();
    await github.close();
  }
});

test("OAuth start 拒绝不在白名单内的 returnUrl", async () => {
  const server = await startRemoteServer({
    port: 0,
    host: "127.0.0.1",
    dbPath: ":memory:",
    secret: "oauth-flow-secret-long-enough",
    githubClientId: "client-id-1",
    githubClientSecret: "client-secret-1",
    githubOAuthCallbackUrl: "https://remote.example.test/callback",
    allowedOrigins: ["http://localhost:5199"],
  });
  try {
    const response = await fetch(`${server.url}/api/v1/auth/github/oauth/start`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ returnUrl: "https://evil.example/steal" }),
    });
    assert.equal(response.status, 422);
  } finally {
    await server.close();
  }
});

test("未配置 client secret 时 OAuth start 返回 503，客户端可回落 Device Flow", async () => {
  const server = await startRemoteServer({
    port: 0,
    host: "127.0.0.1",
    dbPath: ":memory:",
    secret: "oauth-flow-secret-long-enough",
    githubClientId: "client-id-1",
  });
  try {
    const response = await fetch(`${server.url}/api/v1/auth/github/oauth/start`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "{}",
    });
    assert.equal(response.status, 503);
  } finally {
    await server.close();
  }
});

test("pollToken 不匹配不会泄露会话", async () => {
  const github = await startFakeGithub();
  const server = await startRemoteServer({
    port: 0,
    host: "127.0.0.1",
    dbPath: ":memory:",
    secret: "oauth-flow-secret-long-enough",
    githubClientId: "client-id-1",
    githubClientSecret: "client-secret-1",
    githubOAuthCallbackUrl: "https://remote.example.test/callback",
    githubOAuthTokenBaseUrl: github.base,
    githubVerifier: async () => ({ providerUserId: "oauth-user", login: "oauth-user" }),
  });
  try {
    const start = await fetch(`${server.url}/api/v1/auth/github/oauth/start`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "{}",
    });
    const challenge = (await start.json()) as { state: string };
    const poll = await fetch(`${server.url}/api/v1/auth/github/oauth/poll`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ state: challenge.state, pollToken: "wrong-token" }),
    });
    assert.equal(poll.status, 404);
  } finally {
    await server.close();
    await github.close();
  }
});
