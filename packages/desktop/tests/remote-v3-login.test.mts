/**
 * 桌面 Host 的 GitHub 登录走服务端 Device Flow 代理：
 * 客户端不再需要 MODE_REMOTE_GITHUB_CLIENT_ID，也不直接访问 github.com。
 */
import assert from "node:assert/strict";
import test from "node:test";

import { RemoteV3GithubLogin } from "../src/host/remoteV3/login.js";

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

test("桌面 Host 走服务端 Device Flow，不依赖本地 client id", async () => {
  let configured: { serverUrl: string; webSessionToken: string } | null = null;
  let polls = 0;
  const calls: string[] = [];
  const fetchImpl = (async (input: string | URL | Request) => {
    const url = String(input);
    calls.push(url);
    if (url.endsWith("/api/v1/auth/github/oauth/start")) {
      return json({ error: { code: "github_oauth_unavailable" } }, 503);
    }
    if (url.endsWith("/api/v1/auth/github/device/start")) {
      return json({
        deviceCode: "device-1",
        userCode: "AB-12-CD",
        verificationUri: "https://github.com/login/device",
        expiresIn: 900,
        interval: 60,
      });
    }
    if (url.endsWith("/api/v1/auth/github/device/poll")) {
      polls += 1;
      return polls === 1
        ? json({ status: "pending" })
        : json({ status: "ok", sessionToken: "session-1" });
    }
    return json({ error: { message: `unexpected ${url}` } }, 500);
  }) as typeof fetch;

  const login = new RemoteV3GithubLogin({
    env: {},
    fetchImpl,
    getServerUrl: () => "https://remote.mymode.cloud",
    configure: async (input) => {
      configured = input;
      return {} as never;
    },
    onPendingChange: () => undefined,
    log: () => undefined,
  });
  try {
    const challenge = await login.begin();
    assert.equal(challenge.kind, "device");
    if (challenge.kind !== "device") throw new Error("expected device challenge");
    assert.equal(challenge.userCode, "AB-12-CD");
    assert.equal(challenge.verificationUri, "https://github.com/login/device");

    await login.poll();
    assert.equal(configured, null, "pending 阶段不应 configure");

    await login.poll();
    assert.deepEqual(configured, {
      serverUrl: "https://remote.mymode.cloud",
      webSessionToken: "session-1",
    });
    assert.equal(
      calls.every((url) => url.startsWith("https://remote.mymode.cloud/")),
      true,
      "登录请求必须走账号服务，不直连 GitHub",
    );
  } finally {
    login.dispose();
  }
});

test("桌面 Host 优先 OAuth 回调，授权后轮询拿到会话", async () => {
  let configured: { serverUrl: string; webSessionToken: string } | null = null;
  let polls = 0;
  const calls: string[] = [];
  const fetchImpl = (async (input: string | URL | Request) => {
    const url = String(input);
    calls.push(url);
    if (url.endsWith("/api/v1/auth/github/oauth/start")) {
      return json({
        authorizationUrl: "https://github.com/login/oauth/authorize?state=state-1",
        state: "state-1",
        pollToken: "poll-1",
        expiresIn: 600,
        interval: 0,
      });
    }
    if (url.endsWith("/api/v1/auth/github/oauth/poll")) {
      polls += 1;
      return polls === 1
        ? json({ status: "pending" })
        : json({ status: "ok", sessionToken: "session-oauth" });
    }
    return json({ error: { message: `unexpected ${url}` } }, 500);
  }) as typeof fetch;

  const login = new RemoteV3GithubLogin({
    env: {},
    fetchImpl,
    getServerUrl: () => "https://remote.mymode.cloud",
    configure: async (input) => {
      configured = input;
      return {} as never;
    },
    onPendingChange: () => undefined,
    log: () => undefined,
  });
  try {
    const challenge = await login.begin();
    assert.equal(challenge.kind, "oauth");
    if (challenge.kind !== "oauth") throw new Error("expected oauth challenge");
    assert.match(challenge.authorizationUrl, /login\/oauth\/authorize/u);

    await login.poll();
    assert.equal(configured, null, "pending 阶段不应 configure");
    await login.poll();
    assert.deepEqual(configured, {
      serverUrl: "https://remote.mymode.cloud",
      webSessionToken: "session-oauth",
    });
    assert.equal(
      calls.some((url) => url.endsWith("/api/v1/auth/github/device/start")),
      false,
      "OAuth 可用时不应启动 Device Flow",
    );
  } finally {
    login.dispose();
  }
});
