/**
 * 网页门户（docs/specs/cloud-personal-accounts.md §手机/浏览器门户）：
 * 静态页、会话 Cookie、GitHub Device Flow relay 代理。
 * 使用 :memory: SQLite 与 mock GitHub；每个用例用唯一 x-forwarded-for 避让共享限速桶。
 */
import assert from "node:assert/strict";
import { createServer } from "node:http";
import test from "node:test";

import { startRelayServer, type RelayServer } from "../src/server.js";

const RELAY_TOKEN = "relay-token";
const ENROLL_TOKEN = "enroll-token";
const PUBLIC_BASE = "https://relay.example.test";

let ipCounter = 0;
function nextIp(): string {
  ipCounter += 1;
  return `10.2.0.${ipCounter}`;
}

async function withRelay(
  cloud: Record<string, unknown>,
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

async function post(
  port: number,
  path: string,
  body: unknown,
  token?: string,
): Promise<{ status: number; body: Record<string, unknown>; setCookie: string }> {
  const response = await fetch(`http://127.0.0.1:${port}${path}`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-forwarded-for": nextIp(),
      ...(token ? { authorization: `Bearer ${token}` } : {}),
    },
    body: JSON.stringify(body),
  });
  return {
    status: response.status,
    body: (await response.json()) as Record<string, unknown>,
    setCookie: response.headers.get("set-cookie") ?? "",
  };
}

test("门户静态页：启用控制面时 / 与 /portal 返回 HTML，未启用时 404", async () => {
  const plain = await startRelayServer({
    port: 0,
    token: RELAY_TOKEN,
    publicBaseUrl: PUBLIC_BASE,
    log: () => undefined,
  });
  try {
    const response = await fetch(`http://127.0.0.1:${plain.port}/`);
    assert.equal(response.status, 404);
  } finally {
    await plain.close();
  }

  await withRelay({}, async (relay) => {
    for (const path of ["/", "/portal", "/portal.html"]) {
      const response = await fetch(`http://127.0.0.1:${relay.port}${path}`);
      assert.equal(response.status, 200);
      assert.match(response.headers.get("content-type") ?? "", /text\/html/u);
      const html = await response.text();
      assert.match(html, /Mode Cloud/u);
      assert.match(html, /mode_cloud_session|github\/device/u);
    }
  });
});

test("会话 Cookie：登录下发 HttpOnly Cookie，Cookie 可直接读目录，退出清 Cookie", async () => {
  await withRelay({}, async (relay) => {
    const registered = await post(relay.port, "/v1/auth/register", {
      username: "portal-user",
      password: "password-8",
    });
    assert.equal(registered.status, 200);
    assert.match(registered.setCookie, /mode_cloud_session=/u);
    assert.match(registered.setCookie, /HttpOnly/u);
    assert.match(registered.setCookie, /SameSite=Lax/u);
    assert.match(registered.setCookie, /Secure/u);
    const sessionToken = String(registered.body.sessionToken);

    // 只带 Cookie（不带 Bearer）也能读目录。
    const withCookie = await fetch(`http://127.0.0.1:${relay.port}/v1/devices`, {
      headers: { cookie: `mode_cloud_session=${sessionToken}` },
    });
    assert.equal(withCookie.status, 200);
    const payload = (await withCookie.json()) as { account?: Record<string, unknown> };
    assert.equal(payload.account?.username, "portal-user");

    // 退出：清 Cookie 且会话失效。
    const loggedOut = await fetch(`http://127.0.0.1:${relay.port}/v1/auth/logout`, {
      method: "POST",
      headers: { authorization: `Bearer ${sessionToken}` },
    });
    assert.equal(loggedOut.status, 200);
    assert.match(loggedOut.headers.get("set-cookie") ?? "", /Max-Age=0/u);
    const afterLogout = await fetch(`http://127.0.0.1:${relay.port}/v1/devices`, {
      headers: { cookie: `mode_cloud_session=${sessionToken}` },
    });
    assert.equal(afterLogout.status, 401);
  });
});

test("GitHub Device Flow 代理：start/poll、pending、一次性 loginId、未配置 503", async () => {
  let deviceCodeCalls = 0;
  const github = createServer((req, res) => {
    let raw = "";
    req.on("data", (chunk) => {
      raw += chunk;
    });
    req.on("end", () => {
      const url = req.url ?? "";
      if (url === "/login/device/code") {
        deviceCodeCalls += 1;
        res.writeHead(200, { "content-type": "application/json" });
        res.end(
          JSON.stringify({
            device_code: deviceCodeCalls === 1 ? "dc-pending" : "dc-ok",
            user_code: "ABCD-1234",
            verification_uri: "https://github.com/login/device",
            expires_in: 900,
            interval: 5,
          }),
        );
        return;
      }
      if (url === "/login/oauth/access_token") {
        // body 里 device_code 区分 pending 与授权完成。
        const pending = raw.includes("device_code=dc-pending");
        res.writeHead(200, { "content-type": "application/json" });
        res.end(
          JSON.stringify(
            pending ? { error: "authorization_pending" } : { access_token: "gh-token" },
          ),
        );
        return;
      }
      if (url === "/user") {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify({ id: 777001, login: "portal-gh", name: "Portal GH" }));
        return;
      }
      res.writeHead(404);
      res.end("{}");
    });
  });
  await new Promise<void>((resolve) => github.listen(0, "127.0.0.1", resolve));
  const address = github.address();
  const githubPort = typeof address === "object" && address ? address.port : 0;
  const githubBase = `http://127.0.0.1:${githubPort}`;

  try {
    // 未配置 client_id：门户 GitHub 登录关闭。
    await withRelay({}, async (relay) => {
      const disabled = await post(relay.port, "/v1/auth/github/device/start", {});
      assert.equal(disabled.status, 503);
      assert.equal(disabled.body.error, "github_login_disabled");
    });

    await withRelay(
      { githubClientId: "client-1", githubBaseUrl: githubBase, githubApiBaseUrl: githubBase },
      async (relay) => {
        const started = await post(relay.port, "/v1/auth/github/device/start", {});
        assert.equal(started.status, 200);
        assert.equal(started.body.userCode, "ABCD-1234");
        assert.equal(started.body.verificationUri, "https://github.com/login/device");
        const loginId = String(started.body.loginId);
        assert.ok(loginId);

        const pending = await post(relay.port, "/v1/auth/github/device/poll", { loginId });
        assert.equal(pending.status, 202);
        assert.equal(pending.body.status, "pending");

        // 第二次 start 拿到已授权路径。
        const started2 = await post(relay.port, "/v1/auth/github/device/start", {});
        assert.equal(started2.status, 200);
        const loginId2 = String(started2.body.loginId);
        const authorized = await post(relay.port, "/v1/auth/github/device/poll", {
          loginId: loginId2,
        });
        assert.equal(authorized.status, 200, JSON.stringify(authorized.body));
        assert.equal(
          (authorized.body.account as Record<string, unknown>).displayName,
          "Portal GH",
        );
        assert.match(authorized.setCookie, /mode_cloud_session=/u);

        // loginId 一次性：成功后再 poll 401；未知 loginId 也 401。
        const reused = await post(relay.port, "/v1/auth/github/device/poll", {
          loginId: loginId2,
        });
        assert.equal(reused.status, 401);
        assert.equal(reused.body.error, "invalid_login");
        const unknown = await post(relay.port, "/v1/auth/github/device/poll", {
          loginId: "does-not-exist",
        });
        assert.equal(unknown.status, 401);
      },
    );
  } finally {
    await new Promise<void>((resolve) => github.close(() => resolve()));
  }
});
