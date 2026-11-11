import assert from "node:assert/strict";
import test from "node:test";

import {
  WebAccountClient,
  loadOrCreateWebProfile,
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
  const persisted = JSON.parse(store.map.get("mode.remote.v3.web-profile")!) as {
    accessToken: string;
    refreshToken: string;
  };
  assert.equal(persisted.accessToken, "access-2");
  assert.equal(persisted.refreshToken, "refresh-2");
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
