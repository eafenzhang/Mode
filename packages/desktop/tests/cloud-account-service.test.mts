/**
 * Mode Cloud 账号 Host 服务：绑定、目录、6 位码配对、失败语义。
 * 全部使用假凭据/Bot 服务与 mock fetch；不访问真实云端。
 */
import assert from "node:assert/strict";
import { verify } from "node:crypto";
import test from "node:test";

import { Emitter } from "@mode/rpc";
import {
  IBotsService,
  ICredentialService,
  ServiceCollection,
} from "@mode/services";
import {
  LAN_ACCESS_RELAY_TUNNEL_ID_CREDENTIAL_KEY,
  LAN_ACCESS_SERVER_ID_CREDENTIAL_KEY,
  buildLanPeerMetaKey,
  buildLanPeerTokenKey,
  parseLanPeerMeta,
} from "@mode/shared";

import { createCloudAccountHostService } from "../src/host/cloudAccountService.js";

const BASE_URL = "https://cloud.example.test";
const RELAY_TUNNEL_ID = "tunnel-0000000001";
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

function createFakeBots(providerUserId: string | null): IBotsService {
  return {
    listBots: async () =>
      providerUserId
        ? [
            {
              id: "bot-1",
              enabled: true,
              provider: "weixin",
              providerUserId,
              displayName: "微信用户",
            },
          ]
        : [],
  } as unknown as IBotsService;
}

function createSyncBots(): {
  bots: IBotsService;
  saved: Array<Record<string, unknown>>;
} {
  const saved: Array<Record<string, unknown>> = [];
  const bot = {
    id: "bot-cloud",
    enabled: true,
    provider: "weixin",
    providerUserId: "wx-user-1",
    displayName: "云端登录 Bot",
    credentialRef: "bot:bot-cloud:credential",
  };
  const bots = {
    listBots: async () => [bot],
    saveBot: async (params: { bot: Record<string, unknown> }) => {
      saved.push(params.bot);
      return params.bot;
    },
  } as unknown as IBotsService;
  return { bots, saved };
}

function createEventBots(): {
  bots: IBotsService;
  saved: Array<Record<string, unknown>>;
  fire: () => void;
} {
  const emitter = new Emitter<void>();
  const saved: Array<Record<string, unknown>> = [];
  const bot = {
    id: "bot-auto",
    enabled: true,
    provider: "weixin",
    providerUserId: "wx-user-1",
    displayName: "Auto Bot",
    credentialRef: "bot:bot-auto:credential",
  };
  const bots = {
    listBots: async () => [bot],
    saveBot: async (params: { bot: Record<string, unknown> }) => {
      saved.push(params.bot);
      return params.bot;
    },
    onDidChange: emitter.event,
  } as unknown as IBotsService;
  return { bots, saved, fire: () => emitter.fire() };
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function installFetch(): () => void {
  const originalFetch = globalThis.fetch;
  let loginPolls = 0;
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(String(input));
    if (url.pathname === "/v1/auth/start") {
      return json({ code: "123456", expiresAt: Date.now() + 300_000 });
    }
    if (url.pathname === "/v1/auth/poll") {
      const body = JSON.parse(String(init?.body)) as {
        code: string;
        device: { id: string; relayTunnelId: string };
      };
      assert.equal(body.code, "123456");
      assert.match(body.device.relayTunnelId, /^[A-Za-z0-9_-]{16,128}$/u);
      loginPolls += 1;
      if (loginPolls === 1) {
        return json({ status: "pending" }, 202);
      }
      return json({
        accountId: "account-login",
        deviceId: body.device.id,
        deviceToken: "login-device-token",
        relayToken: "relay-token-from-login",
      });
    }
    if (url.pathname === "/v1/admin/wechat-bot") {
      assert.equal(new Headers(init?.headers).get("x-mode-enroll-token"), "enroll-token");
      const body = JSON.parse(String(init?.body)) as { token: string; allowedUser: string };
      assert.equal(body.token, "wechat-token");
      assert.equal(body.allowedUser, "wx-user-1");
      return json({ ok: true, wechatLogin: true });
    }
    if (url.pathname === "/v1/devices/enroll") {
      const body = JSON.parse(String(init?.body)) as {
        assertion: { payload: string; signature: string };
        device: { publicKey: string; id: string; relayTunnelId: string };
      };
      assert.equal(new Headers(init?.headers).get("x-mode-enroll-token"), "enroll-token");
      assert.equal(body.device.id, "device-1");
      assert.equal(body.device.relayTunnelId, RELAY_TUNNEL_ID);
      assert.equal(
        verify(
          null,
          Buffer.from(body.assertion.payload, "utf8"),
          body.device.publicKey,
          Buffer.from(body.assertion.signature, "base64"),
        ),
        true,
        "设备签名必须可被公钥验证",
      );
      return json({
        accountId: "account-1",
        deviceId: "device-1",
        deviceToken: "device-token",
      });
    }
    if (url.pathname === "/v1/devices") {
      const authorization = new Headers(init?.headers).get("authorization");
      assert.ok(
        authorization === "Bearer device-token" ||
          authorization === "Bearer login-device-token",
        "目录请求应携带当前设备令牌",
      );
      return json({
        account: { id: "account-1", displayName: "微信用户" },
        devices: [
          {
            id: "device-2",
            name: "Remote Desk",
            platform: "win32",
            version: "1.0.0",
            online: true,
            relayUrl: `${BASE_URL}/d/${REMOTE_TUNNEL_ID}`,
          },
        ],
      });
    }
    if (url.pathname === `/d/${REMOTE_TUNNEL_ID}/api/lan/pair`) {
      assert.equal(url.protocol, "https:");
      const body = JSON.parse(String(init?.body)) as { code: string };
      assert.equal(body.code, "A1B2C3");
      return json({ token: "remote-token", serverId: "remote-server", name: "Remote Desk" });
    }
    return new Response("not found", { status: 404 });
  }) as typeof fetch;
  return () => {
    globalThis.fetch = originalFetch;
  };
}

async function createService(options: {
  credentials: ICredentialService;
  bots: IBotsService;
  env: NodeJS.ProcessEnv;
  getLanAccessServer?: () => {
    getState(): { port: number | null };
    startRelayTunnel(token: string): void;
  } | null;
}) {
  const services = new ServiceCollection();
  services.register(ICredentialService, options.credentials);
  services.register(IBotsService, options.bots);
  return await createCloudAccountHostService({
    services,
    log: () => undefined,
    env: options.env,
    // 单测隔离本机部署文件；部署文件读取由 cloud-deployment-config.test 覆盖。
    deploymentConfig: {},
    ...(options.getLanAccessServer ? { getLanAccessServer: options.getLanAccessServer } : {}),
  });
}

test("绑定 + 目录 + 6 位码配对：自动写入公网端点", async () => {
  const restoreFetch = installFetch();
  const { credentials, store } = createFakeCredentials();
  store.set(LAN_ACCESS_SERVER_ID_CREDENTIAL_KEY, "device-1");
  store.set(LAN_ACCESS_RELAY_TUNNEL_ID_CREDENTIAL_KEY, RELAY_TUNNEL_ID);
  const service = await createService({
    credentials,
    bots: createFakeBots("wx-user-1"),
    env: { MODE_CLOUD_ENROLL_TOKEN: "enroll-token", MODE_CLOUD_BASE_URL: BASE_URL },
  });
  try {
    const state = await service.bind();
    assert.equal(state.status, "bound");
    assert.equal(state.account?.id, "account-1");
    assert.equal(state.devices?.[0]?.id, "device-2");
    assert.equal(store.get("cloud:device:token"), "device-token");

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

test("没有绑定的微信 Bot 时拒绝绑定", async () => {
  const { credentials } = createFakeCredentials();
  credentials.save(LAN_ACCESS_SERVER_ID_CREDENTIAL_KEY, "device-1");
  credentials.save(LAN_ACCESS_RELAY_TUNNEL_ID_CREDENTIAL_KEY, RELAY_TUNNEL_ID);
  const service = await createService({
    credentials,
    bots: createFakeBots(null),
    env: { MODE_CLOUD_ENROLL_TOKEN: "enroll-token", MODE_CLOUD_BASE_URL: BASE_URL },
  });
  try {
    await assert.rejects(service.bind(), /微信/u);
  } finally {
    await service.dispose();
  }
});

test("云端 Bot 登录：pending → bound，保存账号/设备/中继令牌并启动隧道", async () => {
  const restoreFetch = installFetch();
  const { credentials, store } = createFakeCredentials();
  store.set(LAN_ACCESS_SERVER_ID_CREDENTIAL_KEY, "device-login");
  const tunnelStarts: string[] = [];
  let tunnelStarted = false;
  const service = await createService({
    credentials,
    bots: createFakeBots(null),
    env: { MODE_CLOUD_BASE_URL: BASE_URL },
    getLanAccessServer: () => ({
      getState: () => ({ port: 45880 }),
      startRelayTunnel: (token: string) => {
        if (!tunnelStarted) {
          tunnelStarted = true;
          tunnelStarts.push(token);
        }
      },
    }),
  });
  try {
    const challenge = await service.beginWechatLogin();
    assert.equal(challenge.code, "123456");
    const pending = await service.pollWechatLogin();
    assert.equal(pending.status, "pending");
    const bound = await service.pollWechatLogin();
    assert.equal(bound.status, "bound");
    assert.equal(store.get("cloud:device:token"), "login-device-token");
    assert.equal(store.get("cloud:relay-token"), "relay-token-from-login");
    assert.deepEqual(tunnelStarts, ["relay-token-from-login"]);
    assert.match(
      store.get(LAN_ACCESS_RELAY_TUNNEL_ID_CREDENTIAL_KEY) ?? "",
      /^[A-Za-z0-9_-]{16,128}$/u,
    );
  } finally {
    await service.dispose();
    restoreFetch();
  }
});

test("同步云端登录 Bot：上传 token 并停用本地 Bot", async () => {
  const restoreFetch = installFetch();
  const { credentials, store } = createFakeCredentials();
  store.set("bot:bot-cloud:credential", "wechat-token");
  const { bots, saved } = createSyncBots();
  const service = await createService({
    credentials,
    bots,
    env: { MODE_CLOUD_ENROLL_TOKEN: "enroll-token", MODE_CLOUD_BASE_URL: BASE_URL },
  });
  try {
    const candidates = await service.listCloudBotCandidates();
    assert.deepEqual(candidates, [
      {
        id: "bot-cloud",
        name: "云端登录 Bot",
        boundUser: "wx-user-1",
        enabled: true,
        hasToken: true,
      },
    ]);
    const state = await service.syncCloudLoginBot({ botId: "bot-cloud" });
    assert.equal(state.status, "unbound");
    assert.equal(saved.length, 1);
    assert.equal(saved[0]?.enabled, false);
  } finally {
    await service.dispose();
    restoreFetch();
  }
});

test("绑定 Bot 后自动登录并同步云端，随后停用本地 Bot", async () => {
  const restoreFetch = installFetch();
  const { credentials, store } = createFakeCredentials();
  store.set(LAN_ACCESS_SERVER_ID_CREDENTIAL_KEY, "device-1");
  store.set(LAN_ACCESS_RELAY_TUNNEL_ID_CREDENTIAL_KEY, RELAY_TUNNEL_ID);
  store.set("bot:bot-auto:credential", "wechat-token");
  const { bots, saved, fire } = createEventBots();
  const service = await createService({
    credentials,
    bots,
    env: { MODE_CLOUD_ENROLL_TOKEN: "enroll-token", MODE_CLOUD_BASE_URL: BASE_URL },
  });
  try {
    fire();
    const deadline = Date.now() + 3_000;
    while (saved.length === 0 && Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    assert.equal(saved.length, 1, "自动同步应停用本地 Bot");
    assert.equal(saved[0]?.enabled, false);
    assert.equal(store.get("cloud:device:token"), "device-token");
  } finally {
    await service.dispose();
    restoreFetch();
  }
});

test("Host 启动时已绑定 Bot 也会自动登录（不依赖 onDidChange）", async () => {
  const restoreFetch = installFetch();
  const { credentials, store } = createFakeCredentials();
  store.set(LAN_ACCESS_SERVER_ID_CREDENTIAL_KEY, "device-1");
  store.set(LAN_ACCESS_RELAY_TUNNEL_ID_CREDENTIAL_KEY, RELAY_TUNNEL_ID);
  store.set("bot:bot-auto:credential", "wechat-token");
  // 刻意不调用 fire()：模拟 Bot 在上一个 Host 进程里就已绑定。
  const { bots, saved } = createEventBots();
  const service = await createService({
    credentials,
    bots,
    env: { MODE_CLOUD_ENROLL_TOKEN: "enroll-token", MODE_CLOUD_BASE_URL: BASE_URL },
  });
  try {
    const deadline = Date.now() + 3_000;
    while (store.get("cloud:device:token") === undefined && Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    assert.equal(store.get("cloud:device:token"), "device-token");
    assert.equal(saved.length, 1, "自动同步应停用本地 Bot");
    assert.equal(saved[0]?.enabled, false);
  } finally {
    await service.dispose();
    restoreFetch();
  }
});

test("未配置 enrollment token 时仍为 unbound（微信登录可用）；部署密钥绑定被拒绝", async () => {
  const { credentials } = createFakeCredentials();
  const withoutEnrollment = await createService({
    credentials,
    bots: createFakeBots("wx-user-1"),
    env: {},
  });
  try {
    const state = await withoutEnrollment.getState();
    // 微信 6 位码登录不依赖部署密钥，未配置时不能把整个云账号判成不可用。
    assert.equal(state.status, "unbound");
    assert.equal(state.enrollmentConfigured, false);
    assert.equal(state.boundWeixinName, "微信用户");
    await assert.rejects(withoutEnrollment.bind(), /MODE_CLOUD_ENROLL_TOKEN/u);
  } finally {
    await withoutEnrollment.dispose();
  }

  const unbound = await createService({
    credentials,
    bots: createFakeBots("wx-user-1"),
    env: { MODE_CLOUD_ENROLL_TOKEN: "enroll-token" },
  });
  try {
    const state = await unbound.getState();
    assert.equal(state.status, "unbound");
    assert.equal(state.enrollmentConfigured, true);
    assert.equal(state.boundWeixinName, "微信用户");
  } finally {
    await unbound.dispose();
  }
});
