import assert from "node:assert/strict";
import test from "node:test";

import {
  WebAccountClient,
  deleteWebProfile,
  listWebProfiles,
  loadOrCreateWebProfile,
  saveWebProfile,
  webProfileKey,
  type WebCredentialStore,
} from "../src/web/index.js";

function createStore(): WebCredentialStore & { map: Map<string, string> } {
  const map = new Map<string, string>();
  return {
    map,
    get: async (key) => map.get(key) ?? null,
    set: async (key, value) => {
      map.set(key, value);
    },
    delete: async (key) => {
      map.delete(key);
    },
  };
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

test("Web profile 首次生成身份，重复加载沿用 deviceId 与私钥", async () => {
  const store = createStore();
  const first = await loadOrCreateWebProfile(store, { serverUrl: "https://remote.example.com" });
  assert.equal(first.deviceId.length > 0, true);
  assert.equal(first.privateKey.length > 0, true);
  const second = await loadOrCreateWebProfile(store, { serverUrl: "https://remote.example.com" });
  assert.equal(second.deviceId, first.deviceId);
  assert.equal(second.privateKey, first.privateKey);
});

test("Web 账号登录注册设备，401 时刷新 token 后重试目录", async () => {
  const store = createStore();
  let registerCalls = 0;
  let refreshCalls = 0;
  let listCalls = 0;
  let registeredIdentityKey = "";
  const fetchImpl = (async (input: string | URL | Request, init?: RequestInit) => {
    const url = String(input);
    if (url.endsWith("/api/v1/auth/github")) {
      return json({ sessionToken: "web-session" });
    }
    if (url.endsWith("/api/v1/devices") && init?.method === "POST") {
      registerCalls += 1;
      const body = JSON.parse(String(init.body)) as {
        device: { deviceId: string; identityKey: string; platform: string };
      };
      registeredIdentityKey = body.device.identityKey;
      assert.equal(body.device.platform, "web");
      return json(
        {
          accessToken: "access-1",
          refreshToken: "refresh-1",
          expiresIn: 3600,
          accountId: "acct-1",
        },
        201,
      );
    }
    if (url.endsWith("/api/v1/devices/refresh")) {
      refreshCalls += 1;
      return json({ accessToken: "access-2", refreshToken: "refresh-2" });
    }
    if (url.endsWith("/api/v1/devices") && !init?.method) {
      listCalls += 1;
      if (listCalls === 1) return json({ error: { message: "expired" } }, 401);
      return json({
        items: [
          {
            device: {
              deviceId: "host-1",
              name: "Host",
              role: "host",
              platform: "desktop",
              identityKey: "identity",
            },
            online: true,
            addresses: ["192.168.1.10:45880"],
          },
        ],
      });
    }
    return json({ error: { message: `unexpected ${url}` } }, 500);
  }) as typeof fetch;

  const client = new WebAccountClient({
    store,
    fetchImpl,
    serverUrl: "https://remote.example.com",
    deviceName: "Mode Web",
  });
  const profile = await client.loginWithGithubToken("github-token");
  assert.equal(profile.accessToken, "access-1");
  assert.equal(registerCalls, 1);
  assert.equal(registeredIdentityKey.length, 43);

  const hosts = await client.listHosts();
  assert.equal(refreshCalls, 1);
  assert.equal(hosts.length, 1);
  assert.deepEqual(hosts[0]?.addresses, ["192.168.1.10:45880"]);
  assert.equal(hosts[0]?.identityKey, "identity", "目录投影保留设备公钥");
  const persisted = JSON.parse(store.map.get(webProfileKey("https://remote.example.com"))!) as {
    accessToken: string;
    refreshToken: string;
  };
  assert.equal(persisted.accessToken, "access-2");
  assert.equal(persisted.refreshToken, "refresh-2");
  // /v3 建链用 getProfile() 取 token；刷新结果必须回写到内存，不能返回旧 token。
  assert.equal((await client.getProfile()).accessToken, "access-2");
});

test("Web 账号客户端走服务端 Device Flow 代理并完成设备注册", async () => {
  const store = createStore();
  let polls = 0;
  const fetchImpl = (async (input: string | URL | Request, init?: RequestInit) => {
    const url = String(input);
    if (url.endsWith("/api/v1/auth/github/device/start")) {
      return json({
        deviceCode: "device-1",
        userCode: "ABCD-1234",
        verificationUri: "https://github.com/login/device",
        expiresIn: 900,
        interval: 5,
      });
    }
    if (url.endsWith("/api/v1/auth/github/device/poll")) {
      polls += 1;
      return polls === 1
        ? json({ status: "pending" })
        : json({ status: "ok", sessionToken: "device-session" });
    }
    if (url.endsWith("/api/v1/devices") && init?.method === "POST") {
      return json(
        { accessToken: "device-access", refreshToken: "device-refresh", accountId: "acct" },
        201,
      );
    }
    return json({ error: { message: `unexpected ${url}` } }, 500);
  }) as typeof fetch;

  const client = new WebAccountClient({
    store,
    fetchImpl,
    serverUrl: "https://remote.example.com",
  });
  const challenge = await client.beginGithubDeviceLogin();
  assert.equal(challenge.userCode, "ABCD-1234");
  assert.equal((await client.pollGithubDeviceLogin(challenge.deviceCode)).status, "pending");
  const completed = await client.pollGithubDeviceLogin(challenge.deviceCode);
  assert.equal(completed.status, "ok");
  if (completed.status === "ok") {
    assert.equal(completed.profile.accessToken, "device-access");
  }
});

test("配对：claim/status/redeem 换新 token，cancel 用新凭证", async () => {
  const store = createStore();
  const calls: { url: string; method?: string; auth: string | null }[] = [];
  let currentToken = "device-1";
  const fetchImpl = (async (input: string | URL | Request, init?: RequestInit) => {
    const url = String(input);
    calls.push({
      url,
      method: init?.method,
      auth: new Headers(init?.headers).get("authorization"),
    });
    if (url.endsWith("/api/v1/devices") && init?.method === "POST") {
      return json({ accessToken: "device-1", refreshToken: "refresh-1" });
    }
    if (url.endsWith("/api/v1/pairing/sessions/claim")) {
      assert.equal(new Headers(init?.headers).get("authorization"), `Bearer ${currentToken}`);
      return json({
        pairingId: "p-1",
        verificationCode: "01234567",
        hostName: "Desk",
        expiresAt: 123,
      });
    }
    if (url.endsWith("/api/v1/pairing/sessions/p-1/redeem")) {
      currentToken = "device-2";
      return json({
        accessToken: "device-2",
        refreshToken: "refresh-2",
        accessExpiresAt: 10,
        refreshExpiresAt: 20,
      });
    }
    if (url.endsWith("/api/v1/pairing/sessions/p-1/cancel")) {
      return json({ status: "canceled" });
    }
    if (url.endsWith("/api/v1/pairing/sessions/p-1")) {
      return json({ status: "claimed", expiresAt: 123, hostName: "Desk", clientName: "Web" });
    }
    return json({ error: { message: "unexpected" } }, 500);
  }) as typeof fetch;

  const client = new WebAccountClient({
    store,
    serverUrl: "https://remote.example.com",
    deviceName: "Web",
    fetchImpl,
  });
  await client.loginWithSessionToken("web-session");

  const claim = await client.claimPairing("ticket-1");
  assert.deepEqual(claim, {
    pairingId: "p-1",
    verificationCode: "01234567",
    hostName: "Desk",
    expiresAt: 123,
  });
  assert.deepEqual(await client.getPairing("p-1"), {
    status: "claimed",
    expiresAt: 123,
    hostName: "Desk",
    clientName: "Web",
  });

  await client.redeemPairing("p-1");
  assert.equal((await client.getProfile()).accessToken, "device-2");
  const persisted = JSON.parse(store.map.get(webProfileKey("https://remote.example.com"))!) as {
    accessToken: string;
  };
  assert.equal(persisted.accessToken, "device-2", "redeem 必须落盘新凭证");

  await client.cancelPairing("p-1");
  assert.equal(calls.at(-1)?.auth, "Bearer device-2", "cancel 使用轮换后的 token");
});

test("配对：device token 401 时刷新一次再重试", async () => {
  const store = createStore();
  let claimCalls = 0;
  let refreshCalls = 0;
  const fetchImpl = (async (input: string | URL | Request, init?: RequestInit) => {
    const url = String(input);
    if (url.endsWith("/api/v1/devices") && init?.method === "POST") {
      return json({ accessToken: "device-1", refreshToken: "refresh-1" });
    }
    if (url.endsWith("/api/v1/pairing/sessions/claim")) {
      claimCalls += 1;
      const auth = new Headers(init?.headers).get("authorization");
      if (auth === "Bearer device-1") return json({ error: { message: "expired" } }, 401);
      return json({
        pairingId: "p-2",
        verificationCode: "11112222",
        hostName: "Desk",
        expiresAt: 9,
      });
    }
    if (url.endsWith("/api/v1/devices/refresh")) {
      refreshCalls += 1;
      return json({ accessToken: "device-2", refreshToken: "refresh-2" });
    }
    return json({ error: { message: "unexpected" } }, 500);
  }) as typeof fetch;

  const client = new WebAccountClient({
    store,
    serverUrl: "https://remote.example.com",
    deviceName: "Web",
    fetchImpl,
  });
  await client.loginWithSessionToken("web-session");
  const claim = await client.claimPairing("ticket-2");
  assert.equal(claim.pairingId, "p-2");
  assert.equal(claimCalls, 2);
  assert.equal(refreshCalls, 1);
});

test("多账号隔离：不同服务地址各自保存身份，删除互不影响", async () => {
  const store = createStore();
  const first = await loadOrCreateWebProfile(store, { serverUrl: "https://a.example.com" });
  const second = await loadOrCreateWebProfile(store, { serverUrl: "https://b.example.com" });
  assert.notEqual(first.deviceId, second.deviceId);
  assert.notEqual(first.privateKey, second.privateKey);
  assert.deepEqual(
    (await listWebProfiles(store)).map((entry) => entry.serverUrl),
    ["https://a.example.com", "https://b.example.com"],
  );

  const reloadedFirst = await loadOrCreateWebProfile(store, { serverUrl: "https://a.example.com" });
  assert.equal(reloadedFirst.deviceId, first.deviceId, "切回原账号必须复用同一身份");

  await deleteWebProfile(store, "https://a.example.com");
  assert.deepEqual(
    (await listWebProfiles(store)).map((entry) => entry.serverUrl),
    ["https://b.example.com"],
  );
  const reloadedSecond = await loadOrCreateWebProfile(store, { serverUrl: "https://b.example.com" });
  assert.equal(reloadedSecond.deviceId, second.deviceId);
});

test("signOut 只清空 token：设备身份与账号记录保留，可再次登录", async () => {
  const store = createStore();
  const serverUrl = "https://signout.example.com";
  const created = await loadOrCreateWebProfile(store, { serverUrl, deviceName: "Web" });
  await saveWebProfile(store, { ...created, accessToken: "access", refreshToken: "refresh" });

  const client = new WebAccountClient({ store, serverUrl });
  await client.signOut();

  const stored = JSON.parse(store.map.get(webProfileKey(serverUrl))!) as {
    accessToken: string | null;
    refreshToken: string | null;
    deviceId: string;
    privateKey: string;
  };
  assert.equal(stored.accessToken, null);
  assert.equal(stored.refreshToken, null);
  assert.equal(stored.deviceId, created.deviceId, "退出登录不轮换设备身份");
  assert.equal(stored.privateKey, created.privateKey);
  assert.deepEqual(
    (await listWebProfiles(store)).map((entry) => entry.serverUrl),
    [serverUrl],
    "退出后账号仍保留在可选列表",
  );
});

test("旧版单账号 profile 迁移到按服务地址分键", async () => {
  const store = createStore();
  const legacy = {
    serverUrl: "https://legacy.example.com",
    deviceId: "device-legacy",
    deviceName: "Legacy Web",
    privateKey: "pk",
    accessToken: "access",
    refreshToken: "refresh",
  };
  store.map.set("mode.remote.v3.web-profile", JSON.stringify(legacy));

  const profile = await loadOrCreateWebProfile(store, {
    serverUrl: "https://legacy.example.com",
  });
  assert.equal(profile.deviceId, "device-legacy");
  assert.equal(profile.accessToken, "access");
  assert.equal(store.map.has("mode.remote.v3.web-profile"), false, "旧 key 迁移后删除");
  assert.ok(store.map.has(webProfileKey("https://legacy.example.com")));
});

test("Web 服务地址保留网关/反代路径前缀", async () => {
  const { normalizeWebServerUrl } = await import("../src/web/accountSupport.js");
  assert.equal(
    normalizeWebServerUrl("https://nas.example/app/mode/"),
    "https://nas.example/app/mode",
  );
  assert.equal(
    normalizeWebServerUrl("https://remote.example.com/"),
    "https://remote.example.com",
  );
});
