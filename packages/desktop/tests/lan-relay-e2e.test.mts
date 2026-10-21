/**
 * WSS 中继端到端：真实中继 + 隧道客户端 + 实际主控 peer client。
 * 验证 /d/<tunnelId> 路径前缀贯穿 server-info、session POST、信令/媒体 URL。
 */
import assert from "node:assert/strict";
import { createServer } from "node:http";
import test from "node:test";

import { buildLanPeerMetaKey, buildLanPeerTokenKey, parseLanPeerMeta } from "@mode/shared";
import type { ICredentialService } from "@mode/services";

import { startRelayServer, type RelayServer } from "../../cloud-relay/src/server.js";
import {
  startRelayTunnelClient,
  type RelayTunnelClient,
} from "../../cloud-relay/src/tunnelClient.js";
import { pairLanPeer } from "../src/host/lanRemoteAttach.js";
import {
  openRemoteDesktopPeerSession,
  probeRemoteDesktopPeer,
} from "../src/host/remoteDesktopPeerClient.js";

const TUNNEL_ID = "e2e-tunnel-1234567890";

async function waitFor(predicate: () => boolean, timeoutMs = 5_000): Promise<void> {
  const startedAt = Date.now();
  while (!predicate()) {
    if (Date.now() - startedAt > timeoutMs) {
      throw new Error("waitFor timeout");
    }
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
}

function createCredentials(relayPort: number): ICredentialService {
  const store = new Map<string, string>([
    [
      buildLanPeerMetaKey("server-e2e"),
      JSON.stringify({
        endpoints: [
          {
            scheme: "http",
            host: "127.0.0.1",
            port: relayPort,
            kind: "lan",
            path: `/d/${TUNNEL_ID}`,
          },
        ],
      }),
    ],
    [buildLanPeerTokenKey("server-e2e"), "peer-token"],
  ]);
  return {
    load: async (key: string) => store.get(key) ?? null,
    save: async (key: string, value: string) => {
      store.set(key, value);
    },
    delete: async (key: string) => {
      store.delete(key);
    },
    list: async (prefix: string) => [...store.keys()].filter((key) => key.startsWith(prefix)),
  } as unknown as ICredentialService;
}

test("WSS 中继：实际 peer client 经 /d/<id> 完成探测与会话请求", async () => {
  const local = createServer((req, res) => {
    const chunks: Buffer[] = [];
    req.on("data", (chunk: Buffer) => chunks.push(chunk));
    req.on("end", () => {
      if (req.url?.startsWith("/api/server-info")) {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(
          JSON.stringify({
            serverId: "server-e2e",
            name: "E2E Desk",
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
        );
        return;
      }
      if (req.url?.startsWith("/api/remote-desktop/session")) {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify({ ticket: "signal-ticket", mediaTicket: "media-ticket" }));
        return;
      }
      if (req.url?.startsWith("/api/lan/pair")) {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify({ token: "peer-token-2", serverId: "server-e2e-2", name: "E2E" }));
        return;
      }
      res.writeHead(404);
      res.end();
    });
  });
  await new Promise<void>((resolve) => local.listen(0, "127.0.0.1", resolve));
  const localAddress = local.address();
  const localPort = typeof localAddress === "object" && localAddress ? localAddress.port : 0;

  let relay: RelayServer | null = null;
  let tunnel: RelayTunnelClient | null = null;
  try {
    relay = await startRelayServer({
      port: 0,
      token: "relay-token",
      publicBaseUrl: "http://127.0.0.1",
      log: () => undefined,
    });
    tunnel = startRelayTunnelClient({
      relayUrl: `ws://127.0.0.1:${relay.port}/_tunnel`,
      token: "relay-token",
      tunnelId: TUNNEL_ID,
      localBaseUrl: `http://127.0.0.1:${localPort}`,
      log: () => undefined,
      reconnect: false,
    });
    await waitFor(() => relay?.getTunnelIds().includes(TUNNEL_ID) === true);

    const credentials = createCredentials(relay.port);
    const probe = await probeRemoteDesktopPeer(credentials, "server-e2e");
    assert.equal(probe.online, true);
    assert.equal(probe.remoteDesktop, true);
    assert.equal(probe.remoteDesktopNative, true);
    assert.equal(probe.endpointKind, "lan");

    const session = await openRemoteDesktopPeerSession(credentials, "server-e2e");
    assert.equal(session.ok, true);
    if (session.ok) {
      assert.match(
        session.wsUrl,
        new RegExp(`^ws://127\\.0\\.0\\.1:${relay.port}/d/${TUNNEL_ID}/ws/remote-desktop\\?`, "u"),
      );
      assert.match(
        session.mediaUrl,
        new RegExp(
          `^ws://127\\.0\\.0\\.1:${relay.port}/d/${TUNNEL_ID}/ws/remote-desktop-media\\?`,
          "u",
        ),
      );
    }

    // 配对也走同一条中继路径：/d/<id>/api/lan/pair 到达本地 45880。
    await pairLanPeer({
      credentials,
      host: "127.0.0.1",
      port: relay.port,
      code: "ABC123",
      scheme: "http",
      path: `/d/${TUNNEL_ID}`,
    });
    const pairedMeta = parseLanPeerMeta(
      await credentials.load(buildLanPeerMetaKey("server-e2e-2")),
    );
    assert.deepEqual(pairedMeta?.endpoints, [
      {
        scheme: "http",
        host: "127.0.0.1",
        port: relay.port,
        kind: "lan",
        path: `/d/${TUNNEL_ID}`,
      },
    ]);
    assert.equal(await credentials.load(buildLanPeerTokenKey("server-e2e-2")), "peer-token-2");
  } finally {
    tunnel?.close();
    await relay?.close();
    await new Promise<void>((resolve) => {
      (local as unknown as { closeAllConnections?: () => void }).closeAllConnections?.();
      local.close(() => resolve());
    });
  }
});
