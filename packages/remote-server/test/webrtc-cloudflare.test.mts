import assert from "node:assert/strict";
import test from "node:test";

import {
  IceServerProvider,
  buildIceServers,
  fetchCloudflareTurnIceServers,
} from "../src/webrtc.js";

const CLOUDFLARE_CREDENTIALS = {
  iceServers: [
    {
      urls: ["stun:stun.cloudflare.com:3478", "turn:turn.cloudflare.com:3478?transport=udp"],
      username: "cf-user",
      credential: "cf-credential",
    },
  ],
};

test("Cloudflare 凭证请求：URL、鉴权与 ttl 按契约发送，并解析出 iceServers", async () => {
  let captured: { url: string; init: RequestInit | undefined } | null = null;
  const servers = await fetchCloudflareTurnIceServers(
    { keyId: "key/with space", secret: "token-1", ttlSeconds: 900 },
    {
      fetchImpl: (async (url: string, init?: RequestInit) => {
        captured = { url: String(url), init };
        return new Response(JSON.stringify(CLOUDFLARE_CREDENTIALS), { status: 200 });
      }) as unknown as typeof fetch,
    },
  );
  assert.equal(
    captured?.url,
    "https://rtc.live.cloudflare.com/v1/turn/keys/key%2Fwith%20space/credentials/generate",
    "keyId 必须 URL 编码",
  );
  const headers = captured?.init?.headers as Record<string, string>;
  assert.equal(headers.authorization, "Bearer token-1");
  assert.deepEqual(JSON.parse(String(captured?.init?.body)), { ttl: 900 });
  assert.deepEqual(servers, [
    {
      urls: ["stun:stun.cloudflare.com:3478", "turn:turn.cloudflare.com:3478?transport=udp"],
      username: "cf-user",
      credential: "cf-credential",
    },
  ]);
});

test("Cloudflare 官方单对象形状（iceServers 为对象）也能解析", async () => {
  const servers = await fetchCloudflareTurnIceServers(
    { keyId: "k", secret: "s" },
    {
      fetchImpl: (async () =>
        new Response(
          JSON.stringify({
            iceServers: {
              urls: ["stun:stun.cloudflare.com:3478", "turn:turn.cloudflare.com:3478?transport=udp"],
              username: "u",
              credential: "c",
            },
          }),
          { status: 201 },
        )) as unknown as typeof fetch,
    },
  );
  assert.deepEqual(servers, [
    {
      urls: ["stun:stun.cloudflare.com:3478", "turn:turn.cloudflare.com:3478?transport=udp"],
      username: "u",
      credential: "c",
    },
  ]);
});

test("Cloudflare 凭证请求失败或有响应无条目时抛错", async () => {
  await assert.rejects(
    fetchCloudflareTurnIceServers(
      { keyId: "k", secret: "t" },
      { fetchImpl: (async () => new Response("nope", { status: 500 })) as unknown as typeof fetch },
    ),
    /HTTP 500/u,
  );
  await assert.rejects(
    fetchCloudflareTurnIceServers(
      { keyId: "k", secret: "t" },
      {
        fetchImpl: (async () =>
          new Response(JSON.stringify({ iceServers: [] }), { status: 200 })) as unknown as typeof fetch,
      },
    ),
    /缺少 iceServers/u,
  );
});

test("ICE 拼装把 Cloudflare 凭证追加在 STUN/coturn 之后", () => {
  const servers = buildIceServers(
    {
      stunUrls: ["stun:stun.cloudflare.com:3478"],
      turn: { urls: ["turn:coturn.example.com:3478"], secret: "coturn-secret" },
    },
    "device-1",
    [{ urls: ["turn:turn.cloudflare.com:3478"], username: "u", credential: "c" }],
  );
  assert.equal(servers?.length, 3);
  assert.deepEqual(servers?.[0]?.urls, ["stun:stun.cloudflare.com:3478"]);
  assert.ok(servers?.[1]?.urls.includes("turn:coturn.example.com:3478"));
  assert.ok(servers?.[1]?.username?.endsWith(":device-1"));
  assert.deepEqual(servers?.[2], {
    urls: ["turn:turn.cloudflare.com:3478"],
    username: "u",
    credential: "c",
  });
});

test("提供者缓存 Cloudflare 凭证：有效期内复用，过期后不再下发", async () => {
  let now = 1_000_000;
  let calls = 0;
  const provider = new IceServerProvider(
    {
      stunUrls: ["stun:stun.cloudflare.com:3478"],
      cloudflareTurn: { keyId: "k", secret: "t", ttlSeconds: 600 },
    },
    {
      now: () => now,
      fetchImpl: (async () => {
        calls += 1;
        return new Response(JSON.stringify({ iceServers: CLOUDFLARE_CREDENTIALS.iceServers }), {
          status: 200,
        });
      }) as unknown as typeof fetch,
    },
  );
  await provider.warmup();
  assert.equal(calls, 1);
  assert.equal(provider.get("d")?.length, 2, "STUN + Cloudflare TURN");

  // maybeRefresh 只在临近过期时才发请求；get() 永远不发网络请求。
  provider.maybeRefresh();
  assert.equal(calls, 1, "距过期还早，定时器回调不应请求");
  now += 550_000;
  provider.maybeRefresh();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(calls, 2, "进入 60s 续期窗口后触发刷新");

  now += 601_000;
  const expired = provider.get("d");
  assert.equal(expired?.length, 1, "凭证过期后只剩 STUN，客户端回落 Relay");
});

test("Cloudflare 刷新失败时保留仍在有效期内的旧凭证", async () => {
  let now = 1_000_000;
  let fail = false;
  const provider = new IceServerProvider(
    { cloudflareTurn: { keyId: "k", secret: "t", ttlSeconds: 600 } },
    {
      now: () => now,
      fetchImpl: (async () => {
        if (fail) return new Response("down", { status: 503 });
        return new Response(JSON.stringify({ iceServers: CLOUDFLARE_CREDENTIALS.iceServers }), {
          status: 200,
        });
      }) as unknown as typeof fetch,
    },
  );
  await provider.warmup();
  assert.equal(provider.get("d")?.length, 1);
  fail = true;
  now += 100_000;
  await provider.refresh();
  assert.equal(provider.get("d")?.length, 1, "刷新失败不能清空仍在有效期内的凭证");
});

test("未配置 Cloudflare 时提供者只返回同步拼装的 STUN/coturn", () => {
  const provider = new IceServerProvider({ stunUrls: ["stun:stun.cloudflare.com:3478"] });
  assert.deepEqual(provider.get("d"), [{ urls: ["stun:stun.cloudflare.com:3478"] }]);
  assert.equal(new IceServerProvider(undefined).get("d"), undefined);
});
