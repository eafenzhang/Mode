/**
 * Mode Cloud 个人账号 Host 服务：GitHub Device Flow / 用户名密码登录、会话路径
 * enroll、目录与 6 位码配对（docs/specs/cloud-personal-accounts.md）。
 * 全部使用假凭据与 mock fetch；不访问真实云端。
 */
import assert from "node:assert/strict";
import { verify } from "node:crypto";
import test from "node:test";

import {
  CLOUD_ACCOUNT_ID_CREDENTIAL_KEY,
  CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY,
  CLOUD_RELAY_TOKEN_CREDENTIAL_KEY,
  CLOUD_SESSION_TOKEN_CREDENTIAL_KEY,
  LAN_ACCESS_SERVER_ID_CREDENTIAL_KEY,
  buildLanPeerMetaKey,
  buildLanPeerTokenKey,
  parseLanPeerMeta,
} from "@mode/shared";
import { ICredentialService, ServiceCollection } from "@mode/services";

import { createCloudAccountHostService } from "../src/host/cloudAccountService.js";

const BASE_URL = "https://cloud.example.test";
const REMOTE_TUNNEL_ID = "tunnel-0000000002";

function createFakeCredentials(): {
  credentials: ICredentialService;
  store: Map<string, string>;
} {
  const store = new Map<string, string>();
  const credentials = {
    load: async (key: string) => store.get(key) ?? null,
    save: async (key: string, value: string) => {
      store.set(key, value);
    },
    delete: async (key: string) => {
      store.delete(key);
    },
    list: async (prefix: string) => [...store.keys()].filter((key) => key.startsWith(prefix)),
  } as unknown as ICredentialService;
  return { credentials, store };
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

interface Route {
  method: string;
  test: (url: URL) => boolean;
  handler: (context: { url: URL; init?: RequestInit }) => Response | Promise<Response>;
}

function installFetch(routes: Route[]): () => void {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(String(input));
    const method = (init?.method ?? "GET").toUpperCase();
    for (const route of routes) {
      if (route.method === method && route.test(url)) {
        return await route.handler({ url, init });
      }
    }
    return new Response("not found", { status: 404 });
  }) as typeof fetch;
  return () => {
    globalThis.fetch = originalFetch;
  };
}

function cloudRoute(
  method: "GET" | "POST",
  pathname: string,
  handler: Route["handler"],
): Route {
  return {
    method,
    test: (url) => url.origin === BASE_URL && url.pathname === pathname,
    handler,
  };
}

/** 目录响应：断言携带当前设备令牌。 */
function directoryRoute(expectedToken: string): Route {
  return cloudRoute("GET", "/v1/devices", ({ init }) => {
    const authorization = new Headers(init?.headers).get("authorization");
    assert.equal(authorization, `Bearer ${expectedToken}`, "目录请求应携带当前设备令牌");
    return json({
      account: { id: "account-1", username: "octo", displayName: "Octo Cat" },
      devices: [
        {
          id: "device-2",
          name: "Remote Desk",
          platform: "win32",
          online: true,
          relayUrl: `${BASE_URL}/d/${REMOTE_TUNNEL_ID}`,
        },
      ],
    });
  });
}

/** 会话路径 enroll：验证 Ed25519 断言签名并记录 payload。 */
function enrollRoute(captured: { payloads: Array<Record<string, unknown>> }): Route {
  return cloudRoute("POST", "/v1/devices/enroll", ({ init }) => {
    const body = JSON.parse(String(init?.body)) as {
      assertion: { payload: string; signature: string };
      device: { publicKey: string; id: string; relayTunnelId: string };
      nonce: string;
      issuedAt: number;
    };
    const authorization = new Headers(init?.headers).get("authorization");
    assert.match(authorization ?? "", /^Bearer session-/u, "会话路径 enroll 必须用会话令牌");
    assert.equal(
      verify(
        null,
        Buffer.from(body.assertion.payload, "utf8"),
        body.device.publicKey,
        Buffer.from(body.assertion.signature, "base64"),
      ),
      true,
      "设备签名必须可被上传公钥验证",
    );
    captured.payloads.push(JSON.parse(body.assertion.payload) as Record<string, unknown>);
    return json({
      accountId: "account-1",
      deviceId: body.device.id,
      deviceToken: "device-token-1",
      relayToken: "relay-token-1",
    });
  });
}

function createRelayServer(): {
  tokens: string[];
  server: {
    getState(): { port: number | null };
    startRelayTunnel(token: string): void;
  };
} {
  const tokens: string[] = [];
  return {
    tokens,
    server: {
      getState: () => ({ port: 45880 }),
      startRelayTunnel: (token: string) => {
        if (!tokens.includes(token)) {
          tokens.push(token);
        }
      },
    },
  };
}

async function createService(options: {
  credentials: ICredentialService;
  env: NodeJS.ProcessEnv;
  getLanAccessServer?: () => {
    getState(): { port: number | null };
    startRelayTunnel(token: string): void;
  } | null;
}) {
  const services = new ServiceCollection();
  services.register(ICredentialService, options.credentials);
  return await createCloudAccountHostService({
    services,
    log: () => undefined,
    env: options.env,
    // 单测隔离本机部署文件；部署文件读取由 cloud-deployment-config 测试覆盖。
    deploymentConfig: {},
    ...(options.getLanAccessServer ? { getLanAccessServer: options.getLanAccessServer } : {}),
  });
}

test("密码注册 → 会话 enroll → 目录 bound，中继令牌启动隧道", async () => {
  const captured = { payloads: [] as Array<Record<string, unknown>> };
  const restoreFetch = installFetch([
    cloudRoute("POST", "/v1/auth/register", ({ init }) => {
      const body = JSON.parse(String(init?.body)) as { username: string; password: string };
      assert.equal(body.username, "octo");
      assert.equal(body.password, "correct horse");
      return json({ sessionToken: "session-1" });
    }),
    enrollRoute(captured),
    directoryRoute("device-token-1"),
  ]);
  const { credentials, store } = createFakeCredentials();
  const { tokens: relayTokens, server } = createRelayServer();
  const service = await createService({
    credentials,
    env: { MODE_CLOUD_BASE_URL: BASE_URL },
    getLanAccessServer: () => server,
  });
  try {
    const state = await service.registerWithPassword({
      username: "octo",
      password: "correct horse",
    });
    assert.equal(state.status, "bound");
    assert.equal(state.account?.id, "account-1");
    assert.equal(state.account?.username, "octo");
    assert.equal(state.devices?.[0]?.id, "device-2");
    assert.equal(store.get(CLOUD_SESSION_TOKEN_CREDENTIAL_KEY), "session-1");
    assert.equal(store.get(CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY), "device-token-1");
    assert.equal(store.get(CLOUD_RELAY_TOKEN_CREDENTIAL_KEY), "relay-token-1");
    assert.equal(store.get(CLOUD_ACCOUNT_ID_CREDENTIAL_KEY), "account-1");
    assert.ok(relayTokens.includes("relay-token-1"), "enroll 成功后应启动中继隧道");
    const payload = captured.payloads[0] as {
      device?: { relayTunnelId?: string };
      account?: unknown;
    };
    assert.equal(payload.account, undefined, "会话路径 enroll 的断言 payload 不能含 account 块");
    assert.match(payload.device?.relayTunnelId ?? "", /^[A-Za-z0-9_-]{16,128}$/u);
  } finally {
    await service.dispose();
    restoreFetch();
  }
});

test("用户名密码登录走同一 enroll 路径并保存会话", async () => {
  const captured = { payloads: [] as Array<Record<string, unknown>> };
  const restoreFetch = installFetch([
    cloudRoute("POST", "/v1/auth/login", ({ init }) => {
      const body = JSON.parse(String(init?.body)) as { username: string; password: string };
      assert.equal(body.username, "octo");
      assert.equal(body.password, "another pass");
      return json({ sessionToken: "session-2" });
    }),
    enrollRoute(captured),
    directoryRoute("device-token-1"),
  ]);
  const { credentials, store } = createFakeCredentials();
  const service = await createService({
    credentials,
    env: { MODE_CLOUD_BASE_URL: BASE_URL },
  });
  try {
    const state = await service.loginWithPassword({ username: "octo", password: "another pass" });
    assert.equal(state.status, "bound");
    assert.equal(store.get(CLOUD_SESSION_TOKEN_CREDENTIAL_KEY), "session-2");
    assert.equal(store.get(CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY), "device-token-1");
    assert.equal(captured.payloads.length, 1, "登录后必须完成本机设备 enroll");
  } finally {
    await service.dispose();
    restoreFetch();
  }
});

test("GitHub Device Flow：begin 出码，poll 从 pending 到 bound", async () => {
  const captured = { payloads: [] as Array<Record<string, unknown>> };
  let tokenPolls = 0;
  const restoreFetch = installFetch([
    {
      method: "POST",
      test: (url) => url.origin === "https://github.com" && url.pathname === "/login/device/code",
      handler: ({ init }) => {
        const params = new URLSearchParams(String(init?.body));
        assert.equal(params.get("client_id"), "test-client-id");
        assert.equal(params.get("scope"), "read:user");
        return json({
          device_code: "dc-1",
          user_code: "ABCD-1234",
          verification_uri: "https://github.com/login/device",
          interval: 0,
          expires_in: 600,
        });
      },
    },
    {
      method: "POST",
      test: (url) =>
        url.origin === "https://github.com" && url.pathname === "/login/oauth/access_token",
      handler: ({ init }) => {
        const params = new URLSearchParams(String(init?.body));
        assert.equal(params.get("device_code"), "dc-1");
        tokenPolls += 1;
        if (tokenPolls === 1) {
          return json({ error: "authorization_pending" });
        }
        return json({ access_token: "gho-token", scope: "read:user" });
      },
    },
    cloudRoute("POST", "/v1/auth/github", ({ init }) => {
      const body = JSON.parse(String(init?.body)) as { accessToken: string };
      assert.equal(body.accessToken, "gho-token");
      return json({ sessionToken: "session-gh" });
    }),
    enrollRoute(captured),
    directoryRoute("device-token-1"),
  ]);
  const { credentials, store } = createFakeCredentials();
  const service = await createService({
    credentials,
    env: { MODE_CLOUD_BASE_URL: BASE_URL, MODE_CLOUD_GITHUB_CLIENT_ID: "test-client-id" },
  });
  try {
    const challenge = await service.beginGithubLogin();
    assert.equal(challenge.userCode, "ABCD-1234");
    assert.equal(challenge.verificationUri, "https://github.com/login/device");

    const pending = await service.pollGithubLogin();
    assert.equal(pending.status, "unbound", "用户未授权前应保持等待");
    const bound = await service.pollGithubLogin();
    assert.equal(bound.status, "bound");
    assert.equal(store.get(CLOUD_SESSION_TOKEN_CREDENTIAL_KEY), "session-gh");
    assert.equal(store.get(CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY), "device-token-1");
  } finally {
    await service.dispose();
    restoreFetch();
  }
});

test("未配置 GitHub client_id 时 begin 直接报错", async () => {
  const { credentials } = createFakeCredentials();
  const service = await createService({
    credentials,
    env: { MODE_CLOUD_BASE_URL: BASE_URL },
  });
  try {
    await assert.rejects(service.beginGithubLogin(), /Client ID/u);
  } finally {
    await service.dispose();
  }
});

test("setAccountCredentials：未登录拒绝；已登录携带令牌调用凭据端点", async () => {
  const { credentials, store } = createFakeCredentials();
  await credentials.save(CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY, "device-token-1");
  let credentialsAuth = "";
  const restoreFetch = installFetch([
    cloudRoute("POST", "/v1/auth/credentials", ({ init }) => {
      credentialsAuth = new Headers(init?.headers).get("authorization") ?? "";
      const body = JSON.parse(String(init?.body)) as { username: string; password: string };
      assert.equal(body.username, "octo");
      assert.equal(body.password, "new pass");
      return json({ ok: true });
    }),
    directoryRoute("device-token-1"),
  ]);
  const service = await createService({
    credentials,
    env: { MODE_CLOUD_BASE_URL: BASE_URL },
  });
  try {
    const anonymousCredentials = createFakeCredentials();
    const anonymous = await createService({
      credentials: anonymousCredentials.credentials,
      env: { MODE_CLOUD_BASE_URL: BASE_URL },
    });
    try {
      await assert.rejects(
        anonymous.setAccountCredentials({ username: "octo", password: "x" }),
        /尚未登录/u,
      );
    } finally {
      await anonymous.dispose();
    }

    const state = await service.setAccountCredentials({ username: "octo", password: "new pass" });
    assert.equal(state.status, "bound");
    assert.equal(credentialsAuth, "Bearer device-token-1");
    assert.equal(store.get(CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY), "device-token-1");
  } finally {
    await service.dispose();
    restoreFetch();
  }
});

test("unbind：撤销云端会话并清空本地凭据，回到 unbound", async () => {
  const { credentials, store } = createFakeCredentials();
  store.set(CLOUD_SESSION_TOKEN_CREDENTIAL_KEY, "session-1");
  store.set(CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY, "device-token-1");
  store.set(CLOUD_RELAY_TOKEN_CREDENTIAL_KEY, "relay-token-1");
  store.set(CLOUD_ACCOUNT_ID_CREDENTIAL_KEY, "account-1");
  let logoutAuth = "";
  const restoreFetch = installFetch([
    cloudRoute("POST", "/v1/auth/logout", ({ init }) => {
      logoutAuth = new Headers(init?.headers).get("authorization") ?? "";
      return json({ ok: true });
    }),
  ]);
  const service = await createService({
    credentials,
    env: { MODE_CLOUD_BASE_URL: BASE_URL },
  });
  try {
    const state = await service.unbind();
    assert.equal(state.status, "unbound");
    assert.equal(logoutAuth, "Bearer session-1", "unbind 应先用会话撤销云端登录");
    for (const key of [
      CLOUD_SESSION_TOKEN_CREDENTIAL_KEY,
      CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY,
      CLOUD_RELAY_TOKEN_CREDENTIAL_KEY,
      CLOUD_ACCOUNT_ID_CREDENTIAL_KEY,
    ]) {
      assert.equal(store.get(key), undefined, `unbind 必须清除 ${key}`);
    }
  } finally {
    await service.dispose();
    restoreFetch();
  }
});

test("设备令牌存在但目录不可达时进入 error 状态", async () => {
  const { credentials } = createFakeCredentials();
  await credentials.save(CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY, "device-token-1");
  const restoreFetch = installFetch([
    cloudRoute("GET", "/v1/devices", () => json({ error: "boom" }, 500)),
  ]);
  const service = await createService({
    credentials,
    env: { MODE_CLOUD_BASE_URL: BASE_URL },
  });
  try {
    const state = await service.getState();
    assert.equal(state.status, "error");
    assert.match(state.error ?? "", /HTTP 500/u);
  } finally {
    await service.dispose();
    restoreFetch();
  }
});

test("pairDevice：云设备经 HTTPS 中继完成 6 位码配对", async () => {
  const { credentials, store } = createFakeCredentials();
  await credentials.save(CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY, "device-token-1");
  await credentials.save(LAN_ACCESS_SERVER_ID_CREDENTIAL_KEY, "device-1");
  const restoreFetch = installFetch([
    directoryRoute("device-token-1"),
    {
      method: "POST",
      test: (url) => url.pathname === `/d/${REMOTE_TUNNEL_ID}/api/lan/pair`,
      handler: ({ url, init }) => {
        assert.equal(url.protocol, "https:", "云设备配对必须走 HTTPS");
        const body = JSON.parse(String(init?.body)) as { code: string };
        assert.equal(body.code, "A1B2C3");
        return json({ token: "remote-token", serverId: "remote-server", name: "Remote Desk" });
      },
    },
  ]);
  const service = await createService({
    credentials,
    env: { MODE_CLOUD_BASE_URL: BASE_URL },
  });
  try {
    await assert.rejects(
      service.pairDevice({ deviceId: "device-2", code: "abc" }),
      /6 位/u,
      "非配对码格式必须直接拒绝",
    );
    await assert.rejects(
      service.pairDevice({ deviceId: "missing", code: "A1B2C3" }),
      /找不到/u,
      "目录里不存在的设备必须拒绝",
    );
    const paired = await service.pairDevice({ deviceId: "device-2", code: "a1b2c3" });
    assert.equal(paired.token, "remote-token");
    assert.equal(store.get(buildLanPeerTokenKey("remote-server")), "remote-token");
    const meta = parseLanPeerMeta(store.get(buildLanPeerMetaKey("remote-server")) ?? null);
    assert.ok(meta);
    assert.equal(meta.endpoints[0]?.kind, "public");
    assert.equal(meta.endpoints[0]?.host, "cloud.example.test");
    assert.equal(meta.endpoints[0]?.path, `/d/${REMOTE_TUNNEL_ID}`);
  } finally {
    await service.dispose();
    restoreFetch();
  }
});
