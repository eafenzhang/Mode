import {
  sortLanPeerEndpoints,
  type LanDiscoveredPeer,
  type LanPairedPeer,
  type LanPeerEndpoint,
  type RemoteTarget,
} from "@mode/shared";

import type { RemoteDesktopDeviceProbe } from "@/remote-desktop/deviceAvailability.js";
import type { ModeAgentServerRecord } from "@/remote-desktop/modeAgentPairing.js";

/**
 * 远程控制统一设备模型（docs/specs/remote-control-device-model.md）：
 * Device 只有一层“设备”，Endpoint 是唯一地址事实源，Channel 表达可用能力。
 * 这里只做投影，不持有会话状态；会话仍归 registry / remote desktop controller。
 */
interface RemoteDeviceChannels {
  workspace: boolean;
  desktop: boolean;
  conversation: boolean;
}

interface RemoteDeviceBase {
  id: string;
  name: string;
  endpoints: LanPeerEndpoint[];
  channels: RemoteDeviceChannels;
}

export type RemoteDevice =
  | (RemoteDeviceBase & {
      origin: "lan";
      peer: LanPairedPeer;
      probe?: RemoteDesktopDeviceProbe;
    })
  | (RemoteDeviceBase & { origin: "discovered"; peer: LanDiscoveredPeer })
  | (RemoteDeviceBase & { origin: "mode-agent"; record: ModeAgentServerRecord });

function lanPeerEndpoints(peer: LanPairedPeer): LanPeerEndpoint[] {
  if (peer.endpoints?.length) {
    return sortLanPeerEndpoints(peer.endpoints);
  }
  return peer.host && peer.port > 0
    ? [{ scheme: "http", host: peer.host, port: peer.port, kind: "lan" }]
    : [];
}

function lanChannels(probe: RemoteDesktopDeviceProbe | undefined): RemoteDeviceChannels {
  const desktop =
    probe?.status === "ok" &&
    probe.capability.online &&
    probe.capability.remoteDesktop &&
    probe.capability.remoteDesktopNative;
  // 远程工作区与远程桌面共用 LAN 令牌与端点；配对记录存在即支持工作区。
  return { workspace: true, desktop: Boolean(desktop), conversation: false };
}

export function buildRemoteDevices(params: {
  lanPeers: LanPairedPeer[];
  probes: Record<string, RemoteDesktopDeviceProbe>;
  modeAgents: ModeAgentServerRecord[];
  discovered: LanDiscoveredPeer[];
}): RemoteDevice[] {
  const devices: RemoteDevice[] = [];
  for (const peer of params.lanPeers) {
    const endpoints = lanPeerEndpoints(peer);
    devices.push({
      id: peer.serverId,
      origin: "lan",
      name: peer.name?.trim() || peer.host || peer.serverId,
      endpoints,
      channels: lanChannels(params.probes[peer.serverId]),
      peer,
      ...(params.probes[peer.serverId] ? { probe: params.probes[peer.serverId] } : {}),
    });
  }
  for (const record of params.modeAgents) {
    devices.push({
      id: record.serverName,
      origin: "mode-agent",
      name: record.name,
      endpoints: [{ scheme: "http", host: record.host, port: record.port, kind: "lan" }],
      channels: { workspace: false, desktop: false, conversation: true },
      record,
    });
  }
  for (const peer of params.discovered) {
    devices.push({
      id: peer.serverId,
      origin: "discovered",
      name: peer.name?.trim() || peer.host,
      endpoints: [{ scheme: "http", host: peer.host, port: peer.port, kind: "lan" }],
      channels: { workspace: false, desktop: false, conversation: false },
      peer,
    });
  }
  return devices;
}

/** LAN 配对记录 → 远程工作区目标；端点列表原样带入，identity 仍取主端点。 */
export function lanPeerToRemoteTarget(
  peer: LanPairedPeer,
): Extract<RemoteTarget, { kind: "lan" }> | null {
  const endpoints = lanPeerEndpoints(peer);
  const primary = endpoints[0];
  const host = peer.host || primary?.host;
  const port = peer.port || primary?.port;
  if (!host || !port) {
    return null;
  }
  return {
    kind: "lan",
    host,
    port,
    ...(primary?.scheme ? { scheme: primary.scheme } : {}),
    ...(primary?.path ? { path: primary.path } : {}),
    endpoints,
    ...(peer.serverId ? { serverId: peer.serverId } : {}),
    ...(peer.name ? { serverName: peer.name } : {}),
  };
}

