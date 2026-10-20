/**
 * 公网配对 fixtures：pairLanPeer 在配对成功后把主端点 + 可选公网端点一起写入 meta；
 * 非 HTTPS 公网地址在发请求前就被拒绝。失效模式：只保存 host/port 或接受 http 公网地址。
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

const server = createHttpServer(servicesStub, 0, {
  serverId: "public-pair-fixture",
  name: "public-pair-fixture",
  lanPairing: {
    redeem: async () => ({ token: "fixture-token", serverId: "public-pair-server" }),
  },
});

try {
  await new Promise<void>((resolve) => server.once("listening", () => resolve()));
  const port = (server.address() as AddressInfo).port;
  const credentials = createCredentialService();

  await pairLanPeer({
    credentials,
    host: "127.0.0.1",
    port,
    code: "ABC123",
    publicBaseUrl: "rd.example.com",
  });
  const meta = parseLanPeerMeta(await credentials.load(buildLanPeerMetaKey("public-pair-server")));
  assert.deepEqual(meta?.endpoints, [
    { scheme: "http", host: "127.0.0.1", port, kind: "lan" },
    { scheme: "https", host: "rd.example.com", port: 443, kind: "public" },
  ]);

  await assert.rejects(
    pairLanPeer({
      credentials,
      host: "127.0.0.1",
      port,
      code: "ABC124",
      publicBaseUrl: "http://rd.example.com",
    }),
    /HTTPS/u,
  );

  // 重复配对：带新公网地址时替换旧公网端点，LAN 端点保留。
  await pairLanPeer({
    credentials,
    host: "127.0.0.1",
    port,
    code: "ABC125",
    publicBaseUrl: "https://rd2.example.com",
  });
  const replaced = parseLanPeerMeta(
    await credentials.load(buildLanPeerMetaKey("public-pair-server")),
  );
  assert.deepEqual(replaced?.endpoints, [
    { scheme: "http", host: "127.0.0.1", port, kind: "lan" },
    { scheme: "https", host: "rd2.example.com", port: 443, kind: "public" },
  ]);

  // 不带公网地址的重复配对保留已配置的公网端点。
  await pairLanPeer({
    credentials,
    host: "127.0.0.1",
    port,
    code: "ABC126",
  });
  const preserved = parseLanPeerMeta(
    await credentials.load(buildLanPeerMetaKey("public-pair-server")),
  );
  assert.deepEqual(preserved?.endpoints, [
    { scheme: "http", host: "127.0.0.1", port, kind: "lan" },
    { scheme: "https", host: "rd2.example.com", port: 443, kind: "public" },
  ]);

  console.log("LAN_PUBLIC_PAIR_OK");
} finally {
  (server as unknown as { closeAllConnections?: () => void }).closeAllConnections?.();
  await new Promise<void>((resolve) => server.close(() => resolve()));
  await new Promise((resolve) => setTimeout(resolve, 100));
}
