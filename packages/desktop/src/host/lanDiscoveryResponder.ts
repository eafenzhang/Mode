/**
 * LAN 发现应答（docs/specs/lan-remote-attach.md）：只回应收到的 UDP 探测，
 * 不做周期广播（少噪声，也少信息外泄）。
 *
 * 从 lanAccessServer 抽出：监听生命周期与发现应答同启停，但职责独立；
 * 同机多实例抢不到发现端口时降级为「仅手动地址可达」，不影响已配对监听。
 */
import { createSocket } from "node:dgram";

import {
  LAN_ACCESS_DISCOVERY_PORT,
  LAN_ACCESS_PROBE,
  buildLanAnnouncementPayload,
} from "@mode/shared";

export async function startLanDiscoveryResponder(params: {
  serverId: string;
  name: string;
  version: string;
  port: number;
  isDisposed: () => boolean;
  warn: (...args: unknown[]) => void;
}): Promise<{ close(): void }> {
  const socket = createSocket({ type: "udp4", reuseAddr: true });
  const announcement = buildLanAnnouncementPayload({
    serverId: params.serverId,
    name: params.name,
    version: params.version,
    port: params.port,
    requiresPairing: true,
    platform: process.platform,
  });
  socket.on("message", (message, rinfo) => {
    if (message.toString("utf8").trim() !== LAN_ACCESS_PROBE || params.isDisposed()) {
      return;
    }
    socket.send(announcement, rinfo.port, rinfo.address, (error) => {
      if (error) {
        params.warn("lan discovery reply failed:", error.message);
      }
    });
  });
  socket.on("error", (error) => {
    params.warn("lan discovery socket error:", error.message);
  });
  await new Promise<void>((resolve) => {
    socket.bind(LAN_ACCESS_DISCOVERY_PORT, "0.0.0.0", () => resolve());
    // 同机多实例时发现端口可能已被占用：降级为「仅手动地址可达」，不影响已配对的监听。
    socket.once("error", () => resolve());
  });
  return {
    close() {
      try {
        socket.close();
      } catch {
        // 关闭未绑定的 socket 会抛错：忽略即可。
      }
    },
  };
}
