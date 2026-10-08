import { createSocket } from "node:dgram";
import { networkInterfaces } from "node:os";
import {
  LAN_ACCESS_DISCOVERY_PORT,
  buildLanProbePayload,
  parseLanAnnouncement,
  type LanDiscoveredPeer,
} from "@mode/shared";

/**
 * 局域网发现（main 侧）：向全局/网段广播地址发一次探测，收集 1.5 秒内的应答。
 * 只发探测、不回周期广播——噪声小，也不会让别人在没被问到时看到我们的存在。
 */

const DISCOVERY_WINDOW_MS = 1_500;

function ipv4ToInt(address: string): number | null {
  const parts = address.split(".");
  if (parts.length !== 4) {
    return null;
  }
  let value = 0;
  for (const part of parts) {
    const byte = Number(part);
    if (!Number.isInteger(byte) || byte < 0 || byte > 255) {
      return null;
    }
    value = (value << 8) | byte;
  }
  return value >>> 0;
}

function intToIpv4(value: number): string {
  return [24, 16, 8, 0].map((shift) => (value >>> shift) & 0xff).join(".");
}

/** 由本机地址与掩码推导定向广播地址（部分系统不转发 255.255.255.255）。 */
function computeBroadcastAddress(address: string, netmask: string): string | null {
  const addressInt = ipv4ToInt(address);
  const maskInt = ipv4ToInt(netmask);
  if (addressInt === null || maskInt === null) {
    return null;
  }
  return intToIpv4((addressInt & maskInt) | (~maskInt >>> 0));
}

function collectBroadcastTargets(): string[] {
  const targets = new Set<string>(["255.255.255.255"]);
  for (const infos of Object.values(networkInterfaces())) {
    for (const info of infos ?? []) {
      if (info.family !== "IPv4" || info.internal || !info.netmask) {
        continue;
      }
      const broadcast = computeBroadcastAddress(info.address, info.netmask);
      if (broadcast) {
        targets.add(broadcast);
      }
    }
  }
  return [...targets];
}

export async function discoverLanPeers(
  options: { timeoutMs?: number } = {},
): Promise<LanDiscoveredPeer[]> {
  const peers = new Map<string, LanDiscoveredPeer>();
  const socket = createSocket({ type: "udp4", reuseAddr: true });
  const windowMs = options.timeoutMs ?? DISCOVERY_WINDOW_MS;

  socket.on("message", (message, rinfo) => {
    const announcement = parseLanAnnouncement(message.toString("utf8"));
    if (!announcement) {
      return;
    }
    const key = `${announcement.serverId}@${rinfo.address}:${announcement.port}`;
    peers.set(key, { ...announcement, host: rinfo.address });
  });

  const bound = new Promise<boolean>((resolve) => {
    socket.once("error", () => resolve(false));
    socket.bind(0, "0.0.0.0", () => resolve(true));
  });
  const ok = await bound;
  if (!ok) {
    try {
      socket.close();
    } catch {
      // 绑定失败时 socket 可能已经不可用。
    }
    return [];
  }

  try {
    socket.setBroadcast(true);
    const payload = Buffer.from(buildLanProbePayload(), "utf8");
    for (const target of collectBroadcastTargets()) {
      socket.send(payload, LAN_ACCESS_DISCOVERY_PORT, target, () => {
        // 单个目标发送失败（例如无该网段）不影响其它目标。
      });
    }
    await new Promise((resolve) => setTimeout(resolve, windowMs));
  } finally {
    try {
      socket.close();
    } catch {
      // 已关闭时忽略。
    }
  }

  return [...peers.values()].sort((left, right) =>
    (left.name ?? left.serverId).localeCompare(right.name ?? right.serverId),
  );
}
