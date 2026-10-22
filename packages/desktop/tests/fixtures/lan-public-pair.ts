/**
 * 公网配对 fixtures：pairLanPeer 在配对成功后把主端点 + 被控端下发的内置中继端点
 * 一起写入 meta；端点按公网优先排序（见 docs/specs/lan-remote-public-endpoints.md）。
 * 失效模式：只保存 host/port、忽略 publicRelay、接受非法 tunnelId，或退回 LAN 优先顺序。
 */
import assert from "node:assert/strict";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { AddressInfo } from "node:net";

// MODE_DATA_BASE_DIR 在 services 模块加载时读取，必须先设再动态导入。
process.env.MODE_DATA_BASE_DIR = await mkdtemp(join(tmpdir(), "mode-lan-public-pair-"));
const { createCredentialService } = await import("@mode/services/node");
const { createHttpServer } = await import("@mode/server");
const { buildLanPeerMetaKey, parseLanPeerMeta } = await import("@mode/shared");
const { pairLanPeer } = await import("../../src/host/lanRemoteAttach.js");

const servicesStub = {
  getOptional: () => undefined,
  exposeOnChannelServer: () => undefined,
} as never;

// 每个配对码模拟一台被控端：有的配置了内置中继，有的没有，有的 tunnelId 非法。
const serversByCode: Record<
  string,
  { serverId: string; publicRelay?: { baseUrl: string; tunnelId: string } }
> = {
  ABC123: {
    serverId: "public-pair-server",
    publicRelay: { baseUrl: "https://relay.mymode.cloud", tunnelId: "relayTunnelId0001" },
  },
  ABC125: {
    serverId: "public-pair-server",
    publicRelay: { baseUrl: "https://relay.mymode.cloud", tunnelId: "relayTunnelId0002" },
  },
  ABC126: { serverId: "public-pair-server" },
  ABC127: {
    serverId: "public-pair-invalid",
    // 长度不足 16：主控必须忽略这个公网端点，不能写入凭据。
    publicRelay: { baseUrl: "https://relay.mymode.cloud", tunnelId: "short" },
  },
};

const server = createHttpServer(servicesStub, 0, {
  serverId: "public-pair-fixture",
  name: "public-pair-fixture",
  lanPairing: {
    redeem: async (request) => {
      const entry = serversByCode[request.code.trim().toUpperCase()];
      if (!entry) {
        return null;
      }
      return {
        token: `fixture-token-${entry.serverId}`,
        serverId: entry.serverId,
        ...(entry.publicRelay ? { publicRelay: entry.publicRelay } : {}),
      };
    },
  },
});

try {
  await new Promise<void>((resolve) => server.once("listening", () => resolve()));
  const port = (server.address() as AddressInfo).port;
  const credentials = createCredentialService();

  await pairLanPeer({ credentials, host: "127.0.0.1", port, code: "ABC123" });
  const meta = parseLanPeerMeta(await credentials.load(buildLanPeerMetaKey("public-pair-server")));
  assert.deepEqual(meta?.endpoints, [
    {
      scheme: "https",
      host: "relay.mymode.cloud",
      port: 443,
      kind: "public",
      path: "/d/relayTunnelId0001",
    },
    { scheme: "http", host: "127.0.0.1", port, kind: "lan" },
  ]);

  // 重复配对：被控端下发新的中继路径时替换旧公网端点，LAN 端点保留。
  await pairLanPeer({ credentials, host: "127.0.0.1", port, code: "ABC125" });
  const replaced = parseLanPeerMeta(
    await credentials.load(buildLanPeerMetaKey("public-pair-server")),
  );
  assert.deepEqual(replaced?.endpoints, [
    {
      scheme: "https",
      host: "relay.mymode.cloud",
      port: 443,
      kind: "public",
      path: "/d/relayTunnelId0002",
    },
    { scheme: "http", host: "127.0.0.1", port, kind: "lan" },
  ]);

  // 不带 publicRelay 的重复配对保留已配置的公网端点。
  await pairLanPeer({ credentials, host: "127.0.0.1", port, code: "ABC126" });
  const preserved = parseLanPeerMeta(
    await credentials.load(buildLanPeerMetaKey("public-pair-server")),
  );
  assert.deepEqual(preserved?.endpoints, replaced?.endpoints);

  // 非法 tunnelId：忽略公网端点，仍以 LAN 端点配对成功。
  await pairLanPeer({ credentials, host: "127.0.0.1", port, code: "ABC127" });
  const invalid = parseLanPeerMeta(
    await credentials.load(buildLanPeerMetaKey("public-pair-invalid")),
  );
  assert.deepEqual(invalid?.endpoints, [{ scheme: "http", host: "127.0.0.1", port, kind: "lan" }]);

  console.log("LAN_PUBLIC_PAIR_OK");
} finally {
  (server as unknown as { closeAllConnections?: () => void }).closeAllConnections?.();
  await new Promise<void>((resolve) => server.close(() => resolve()));
  await new Promise((resolve) => setTimeout(resolve, 100));
}
