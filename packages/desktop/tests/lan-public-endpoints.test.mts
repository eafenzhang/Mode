/**
 * 公网远程桌面端点（docs/specs/lan-remote-public-endpoints.md）：
 * 1. 公网地址只接受 HTTPS，旧 meta 迁移为 LAN 端点；
 * 2. 探测按 LAN → 公网回退，会话 URL 由端点 protocol 推导（https → wss）；
 * 3. 对端级错误（如 busy）不向另一个端点重发，避免重复同意窗。
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  buildLanPeerMetaKey,
  buildLanPeerTokenKey,
  formatLanPeerEndpoint,
  normalizeLanPeerMeta,
  parseLanPublicBaseUrl,
} from "@mode/shared";
import type { ICredentialService } from "@mode/services";

import {
  openRemoteDesktopPeerSession,
  probeRemoteDesktopPeer,
} from "../src/host/remoteDesktopPeerClient.js";

function createCredentials(meta: unknown, token = "peer-token"): ICredentialService {
  const store = new Map<string, string>([
    [buildLanPeerMetaKey("server-1"), JSON.stringify(meta)],
    [buildLanPeerTokenKey("server-1"), token],
  ]);
  return {
    load: async (key: string) => store.get(key) ?? null,
    save: async () => undefined,
    delete: async () => undefined,
    list: async (prefix: string) => [...store.keys()].filter((key) => key.startsWith(prefix)),
  } as unknown as ICredentialService;
}

const HYBRID_META = {
  endpoints: [
    { scheme: "http", host: "127.0.0.1", port: 45880, kind: "lan" },
    { scheme: "https", host: "rd.example.com", port: 443, kind: "public" },
  ],
  name: "Desk",
  pairedAt: 1,
};

function serverInfoResponse(): Response {
  return new Response(
    JSON.stringify({
      serverId: "server-1",
      name: "Desk",
      version: "0.0.99",
      protocolVersion: 1,
      authRequired: false,
      workspaces: [],
      capabilities: {
        desktopContinuous: true,
        websocketRpc: true,
        remoteDesktop: true,
        remoteDesktopNative: true,
      },
    }),
    { status: 200, headers: { "content-type": "application/json" } },
  );
}

test("公网地址只接受 HTTPS，并补默认端口", () => {
  assert.deepEqual(parseLanPublicBaseUrl("rd.example.com"), {
    scheme: "https",
    host: "rd.example.com",
    port: 443,
    kind: "public",
  });
  assert.deepEqual(parseLanPublicBaseUrl("https://rd.example.com:8443"), {
    scheme: "https",
    host: "rd.example.com",
    port: 8443,
    kind: "public",
  });
  assert.equal(parseLanPublicBaseUrl("http://rd.example.com"), null);
  assert.equal(parseLanPublicBaseUrl("https://user:pass@rd.example.com"), null);
  assert.equal(parseLanPublicBaseUrl("https://rd.example.com/path"), null);
  assert.equal(parseLanPublicBaseUrl("https://rd.example.com?x=1"), null);
  assert.equal(parseLanPublicBaseUrl(""), null);
  assert.equal(
    formatLanPeerEndpoint({ scheme: "https", host: "rd.example.com", port: 443, kind: "public" }),
    "https://rd.example.com",
  );
});

test("旧 meta 迁移为 LAN 端点，新 meta 按 LAN 优先排序", () => {
  assert.deepEqual(normalizeLanPeerMeta({ host: "192.168.1.2", port: 45880, name: "Desk" }), {
    endpoints: [{ scheme: "http", host: "192.168.1.2", port: 45880, kind: "lan" }],
    name: "Desk",
  });
  assert.deepEqual(normalizeLanPeerMeta(HYBRID_META)?.endpoints, HYBRID_META.endpoints);
  assert.deepEqual(
    normalizeLanPeerMeta({
      endpoints: [HYBRID_META.endpoints[1], HYBRID_META.endpoints[0]],
    })?.endpoints,
    HYBRID_META.endpoints,
    "公网端点即使先写入也要排在 LAN 之后",
  );
  assert.equal(normalizeLanPeerMeta({ host: "", port: 1 }), null);
  // 防御性：凭据被改成 public + http 时不得进入连接路径。
  assert.deepEqual(
    normalizeLanPeerMeta({
      endpoints: [
        { scheme: "https", host: "rd.example.com", port: 443, kind: "public" },
        { scheme: "http", host: "evil.example.com", port: 80, kind: "public" },
      ],
    })?.endpoints,
    [{ scheme: "https", host: "rd.example.com", port: 443, kind: "public" }],
  );
  assert.equal(
    normalizeLanPeerMeta({
      endpoints: [{ scheme: "http", host: "evil.example.com", port: 80, kind: "public" }],
    }),
    null,
  );
});

test("探测与会话：LAN 不可达时回退公网，并用 wss 建链接", async () => {
  const originalFetch = globalThis.fetch;
  const requested: string[] = [];
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const url = new URL(String(input));
    requested.push(`${url.hostname}${url.pathname}`);
    if (url.hostname === "127.0.0.1") {
      throw new TypeError("fetch failed");
    }
    if (url.pathname === "/api/server-info") {
      return serverInfoResponse();
    }
    if (url.pathname === "/api/remote-desktop/session") {
      return new Response(
        JSON.stringify({ ticket: "signal-ticket", mediaTicket: "media-ticket" }),
        {
          status: 200,
          headers: { "content-type": "application/json" },
        },
      );
    }
    return new Response("not found", { status: 404 });
  }) as typeof fetch;
  try {
    const credentials = createCredentials(HYBRID_META);
    const probe = await probeRemoteDesktopPeer(credentials, "server-1");
    assert.deepEqual(
      { online: probe.online, kind: probe.endpointKind, host: probe.endpointHost },
      { online: true, kind: "public", host: "rd.example.com" },
    );

    const session = await openRemoteDesktopPeerSession(credentials, "server-1");
    assert.equal(session.ok, true);
    if (session.ok) {
      assert.match(session.wsUrl, /^wss:\/\/rd\.example\.com\/ws\/remote-desktop\?/u);
      assert.match(session.mediaUrl, /^wss:\/\/rd\.example\.com\/ws\/remote-desktop-media\?/u);
      assert.match(session.wsUrl, /token=peer-token/u);
      assert.doesNotMatch(session.mediaUrl, /token=/u, "mediaUrl 只带 mediaTicket");
    }
    assert.deepEqual(requested, [
      // probeRemoteDesktopPeer：LAN 失败 → 公网成功
      "127.0.0.1/api/server-info",
      "rd.example.com/api/server-info",
      // openRemoteDesktopPeerSession：重新探测（LAN 失败 → 公网成功）→ 只 POST 一次
      "127.0.0.1/api/server-info",
      "rd.example.com/api/server-info",
      "rd.example.com/api/remote-desktop/session",
    ]);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("对端级错误不向另一个端点重发", async () => {
  const originalFetch = globalThis.fetch;
  const requested: string[] = [];
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const url = new URL(String(input));
    requested.push(`${url.hostname}${url.pathname}`);
    if (url.pathname === "/api/server-info") {
      return serverInfoResponse();
    }
    if (url.pathname === "/api/remote-desktop/session") {
      return new Response(JSON.stringify({ code: "busy" }), {
        status: 409,
        headers: { "content-type": "application/json" },
      });
    }
    return new Response("not found", { status: 404 });
  }) as typeof fetch;
  try {
    const session = await openRemoteDesktopPeerSession(createCredentials(HYBRID_META), "server-1");
    assert.equal(session.ok, false);
    if (!session.ok) {
      assert.equal(session.errorCode, "busy");
    }
    assert.equal(
      requested.some((entry) => entry.startsWith("rd.example.com")),
      false,
      "LAN 端点已给出对端级结果，不应再请求公网端点",
    );
    assert.deepEqual(requested, [
      "127.0.0.1/api/server-info",
      "127.0.0.1/api/remote-desktop/session",
    ]);
  } finally {
    globalThis.fetch = originalFetch;
  }
});
