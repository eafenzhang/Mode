/**
 * Tailscale 单播发现夹具：
 * 1) parseTailscaleStatusIps 解析 status --json 样例（Self+Peer 的 IPv4，去重、滤 v6）；
 * 2) discoverLanPeers 注入 runner 返回 ["127.0.0.1"] 时，单播报文应到达
 *    127.0.0.1:45879 的本地应答器并把应答设备收进结果（host=127.0.0.1）。
 * 失效模式（未实现时）：只广播不单播 → 应答器收不到探针 → 结果为空 → 断言失败。
 * 端口被真实 LAN 服务占用时输出 TAILSCALE_DISCOVERY_SKIP，由包装测试按跳过处理。
 */
import { createSocket } from "node:dgram";
import assert from "node:assert/strict";
import {
  LAN_ACCESS_DISCOVERY_PORT,
  LAN_ACCESS_PROBE,
  buildLanAnnouncementPayload,
} from "@mode/shared";
import { discoverLanPeers, parseTailscaleStatusIps } from "../../src/main/desktopLanDiscovery.js";

// ── 1) status --json 解析（纯函数） ──
const sampleStatus = JSON.stringify({
  Version: "1.76.0",
  Self: {
    ID: "self1",
    HostName: "desktop-k1dp6jf",
    TailscaleIPs: ["100.90.96.50", "fd7a:115c:a1e0::1"],
  },
  Peer: {
    "nodekey:abc": {
      HostName: "mx121212",
      TailscaleIPs: ["100.68.49.120", "fd7a:115c:a1e0::2"],
      Online: true,
    },
    "nodekey:def": {
      HostName: "minipc-1",
      TailscaleIPs: ["100.83.113.72"],
      Online: false,
    },
    "nodekey:dup": {
      HostName: "dup",
      TailscaleIPs: ["100.68.49.120"],
      Online: true,
    },
  },
});
const ips = parseTailscaleStatusIps(sampleStatus).sort();
assert.deepEqual(
  ips,
  ["100.68.49.120", "100.83.113.72", "100.90.96.50"],
  "Self+Peer 的 IPv4 去重收集（离线 peer 也保留，滤掉 IPv6）",
);
assert.deepEqual(parseTailscaleStatusIps("not-json"), [], "坏 JSON 返回空");
assert.deepEqual(parseTailscaleStatusIps("{}"), [], "无节点返回空");

// ── 2) 单播发现端到端 ──
const responder = createSocket({ type: "udp4" });
try {
  await new Promise<void>((resolve, reject) => {
    responder.once("error", reject);
    responder.bind(LAN_ACCESS_DISCOVERY_PORT, "127.0.0.1", () => resolve());
  });
} catch {
  console.log("TAILSCALE_DISCOVERY_SKIP");
  process.exit(0);
}

responder.on("message", (message, rinfo) => {
  if (message.toString("utf8") !== LAN_ACCESS_PROBE) {
    return;
  }
  responder.send(
    Buffer.from(
      buildLanAnnouncementPayload({
        serverId: "ts-fixture-server",
        name: "ts-fixture",
        version: "0.0.0",
        port: 45880,
        requiresPairing: true,
        platform: "win32",
      }),
    ),
    rinfo.port,
    rinfo.address,
  );
});

try {
  const peers = await discoverLanPeers({
    timeoutMs: 600,
    tailscaleStatusRunner: async () => ["127.0.0.1"],
  });
  const found = peers.find((peer) => peer.serverId === "ts-fixture-server");
  assert.ok(found, `未发现注入的 tailnet 应答器，peers=${JSON.stringify(peers)}`);
  assert.equal(found.host, "127.0.0.1", "应答 host 应是探针目标地址");
  console.log("TAILSCALE_DISCOVERY_OK");
} finally {
  responder.close();
}
