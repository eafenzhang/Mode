/**
 * Mode Cloud 个人账号 Host 服务（docs/specs/cloud-personal-accounts.md，v2）：
 * 仅 GitHub Device Flow——发起即打开授权页 + Host 自动轮询到 bound；
 * 会话路径 enroll 携带本机访问令牌与地址；connectDevice 目录直连（无配对码）。
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
      account: { id: "account-1", displayName: "Octo Cat" },
      devices: [
        {
          id: "device-2",
          name: "Remote Desk",
          platform: "win32",
          online: true,
          relayUrl: `${BASE_URL}/d/${REMOTE_TUNNEL_ID}`,
          addresses: ["192.168.1.8:47821"],
          accessToken: "remote-device-token-123456",
        },
      ],
    });
  });
}

/** 会话路径 enroll：验证 Ed25519 断言签名并记录 payload 与本机访问令牌。 */
function enrollRoute(captured: {
  payloads: Array<Record<string, unknown>>;
  accessTokens: string[];
  addresses: string[][];
}): Route {
  return cloudRoute("POST", "/v1/devices/enroll", ({ init }) => {
    const body = JSON.parse(String(init?.body)) as {
      assertion: { payload: string; signature: string };
      device: {
        publicKey: string;
        id: string;
        relayTunnelId: string;
        accessToken: string;
        addresses?: string[];
      };
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
    captured.accessTokens.push(body.device.accessToken);
    captured.addresses.push(body.device.addresses ?? []);
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
    getState(): { port: number | null; addresses?: string[] };
    startRelayTunnel(token: string): void;
    ensureCloudPeerToken(): Promise<string>;
  };
} {
  const tokens: string[] = [];
  return {
    tokens,
    server: {
      getState: () => ({ port: 45880, addresses: ["192.168.1.5:45880", "100.100.2.3:45880"] }),
      startRelayTunnel: (token: string) => {
        if (!tokens.includes(token)) {
          tokens.push(token);
        }
      },
      ensureCloudPeerToken: async () => "local-lan-access-token-123456",
    },
  };
}

async function createService(options: {
  credentials: ICredentialService;
  env: NodeJS.ProcessEnv;
  getLanAccessServer?: () => ReturnType<typeof createRelayServer>["server"] | null;
  openExternal?: (url: string) => void;
  loginPollIntervalMs?: number;
}) {
  const services = new ServiceCollection();
  services.register(ICredentialService, options.credentials);
  return await createCloudAccountHostService({
    services,
    log: () => undefined,
    env: options.env,
    // 单测隔离本机部署文件；部署文件读取由 cloud-deployment-config 测试覆盖。
    deploymentConfig: {},
    ...(options.getLanAccessServer
      ? { getLanAccessServer: options.getLanAccessServer }
      : {}),
    ...(options.openExternal ? { openExternal: options.openExternal } : {}),
    ...(options.loginPollIntervalMs ? { loginPollIntervalMs: options.loginPollIntervalMs } : {}),
  });
}

function waitUntil(
  condition: () => boolean,
  timeoutMs = 5_000,
  stepMs = 20,
): Promise<void> {
  return new Promise((resolve, reject) => {
    const startedAt = Date.now();
    const check = (): void => {
      if (condition()) {
        resolve();
        return;
      }
      if (Date.now() - startedAt > timeoutMs) {
        reject(new Error("condition timeout"));
        return;
      }
      setTimeout(check, stepMs);
    };
    check();
  });
}

const BASE_ENV = { MODE_CLOUD_BASE_URL: BASE_URL, MODE_CLOUD_GITHUB_CLIENT_ID: "test-client-id" };

test("GitHub 登录：begin 打开授权页，Host 自动轮询到 bound 并完成 enroll", async () => {
  const captured = {
    payloads: [] as Array<Record<string, unknown>>,
    accessTokens: [] as string[],
    addresses: [] as string[][],
  };
  let tokenPolls = 0;
  const openedUrls: string[] = [];
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
  const { tokens: relayTokens, server } = createRelayServer();
  const service = await createService({
    credentials,
    env: BASE_ENV,
    getLanAccessServer: () => server,
    openExternal: (url) => openedUrls.push(url),
    loginPollIntervalMs: 50,
  });
  try {
    const challenge = await service.beginGithubLogin();
    assert.equal(challenge.userCode, "ABCD-1234");
    // v2 交互：发起登录即自动打开授权页。
    assert.deepEqual(openedUrls, ["https://github.com/login/device"]);

    // Host 自动轮询驱动到 bound，无需手动 poll。
    await waitUntil(() => store.get(CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY) === "device-token-1");
    const state = await service.getState();
    assert.equal(state.status, "bound");
    assert.equal(state.account?.displayName, "Octo Cat");
    assert.equal(state.devices?.[0]?.id, "device-2");
    assert.equal(store.get(CLOUD_SESSION_TOKEN_CREDENTIAL_KEY), "session-gh");
    assert.equal(store.get(CLOUD_RELAY_TOKEN_CREDENTIAL_KEY), "relay-token-1");
    assert.ok(relayTokens.includes("relay-token-1"), "enroll 成功后应启动中继隧道");

    // enroll 请求携带本机 API 令牌与自报地址；断言 payload 无 account 块。
    assert.deepEqual(captured.accessTokens, ["local-lan-access-token-123456"]);
    assert.deepEqual(captured.addresses, [["192.168.1.5:45880", "100.100.2.3:45880"]]);
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

test("未配置 GitHub client_id 时 begin 直接报错；密码接口已不存在", async () => {
  const { credentials } = createFakeCredentials();
  const service = await createService({
    credentials,
    env: { MODE_CLOUD_BASE_URL: BASE_URL },
  });
  try {
    await assert.rejects(service.beginGithubLogin(), /Client ID/u);
    const contract = service as unknown as Record<string, unknown>;
    assert.equal(contract.registerWithPassword, undefined);
    assert.equal(contract.loginWithPassword, undefined);
    assert.equal(contract.setAccountCredentials, undefined);
    assert.equal(typeof contract.connectDevice, "function");
  } finally {
    await service.dispose();
  }
});

test("登录轮询遇到硬错误时停止并携带 loginError", async () => {
  const restoreFetch = installFetch([
    {
      method: "POST",
      test: (url) => url.origin === "https://github.com" && url.pathname === "/login/device/code",
      handler: () =>
        json({
          device_code: "dc-2",
          user_code: "BBBB-2222",
          verification_uri: "https://github.com/login/device",
          interval: 0,
          expires_in: 600,
        }),
    },
    {
      method: "POST",
      test: (url) =>
        url.origin === "https://github.com" && url.pathname === "/login/oauth/access_token",
      handler: () => json({ error: "expired_token" }),
    },
  ]);
  const { credentials } = createFakeCredentials();
  const service = await createService({
    credentials,
    env: BASE_ENV,
    loginPollIntervalMs: 50,
  });
  try {
    const states: Array<string> = [];
    const subscription = service.onDidChange((state) => states.push(state.status));
    await service.beginGithubLogin();
    await waitUntil(() => states.length > 0);
    subscription.dispose();
    const state = await service.getState();
    assert.match(state.loginError ?? "", /过期/u);
    // 再次 begin 前自动轮询已停止：不重复打开浏览器（由用例一的 openedUrls 语义保证）。
  } finally {
    await service.dispose();
    restoreFetch();
  }
});

test("connectDevice：目录直连写入 lan:peer 凭据（无配对码）", async () => {
  const { credentials, store } = createFakeCredentials();
  await credentials.save(CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY, "device-token-1");
  await credentials.save(LAN_ACCESS_SERVER_ID_CREDENTIAL_KEY, "device-1");
  const restoreFetch = installFetch([directoryRoute("device-token-1")]);
  const service = await createService({
    credentials,
    env: BASE_ENV,
  });
  try {
    await assert.rejects(
      service.connectDevice("missing"),
      /找不到/u,
      "目录里不存在的设备必须拒绝",
    );
    const result = await service.connectDevice("device-2");
    assert.equal(result.serverId, "device-2");
    assert.equal(result.name, "Remote Desk");
    assert.equal(store.get(buildLanPeerTokenKey("device-2")), "remote-device-token-123456");
    const meta = parseLanPeerMeta(store.get(buildLanPeerMetaKey("device-2")) ?? null);
    assert.ok(meta);
    assert.equal(meta.endpoints[0]?.kind, "public");
    assert.equal(meta.endpoints[0]?.host, "cloud.example.test");
    assert.equal(meta.endpoints[0]?.path, `/d/${REMOTE_TUNNEL_ID}`);
    const lan = meta.endpoints.find((endpoint) => endpoint.kind === "lan");
    assert.ok(lan, "目录自报地址应生成 LAN 端点");
    assert.equal(lan.host, "192.168.1.8");
    assert.equal(lan.port, 47821);
  } finally {
    await service.dispose();
    restoreFetch();
  }
});

test("connectDevice：目录令牌缺失时提示对端重新登录", async () => {
  const { credentials } = createFakeCredentials();
  await credentials.save(CLOUD_DEVICE_TOKEN_CREDENTIAL_KEY, "device-token-1");
  const restoreFetch = installFetch([
    cloudRoute("GET", "/v1/devices", () =>
      json({
        account: { id: "account-1" },
        devices: [
          {
            id: "device-2",
            name: "Remote Desk",
            platform: "win32",
            online: true,
            relayUrl: `${BASE_URL}/d/${REMOTE_TUNNEL_ID}`,
            tokenAvailable: false,
          },
        ],
      }),
    ),
  ]);
  const service = await createService({
    credentials,
    env: BASE_ENV,
  });
  try {
    await assert.rejects(service.connectDevice("device-2"), /重新登录/u);
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
    env: BASE_ENV,
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
    env: BASE_ENV,
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
