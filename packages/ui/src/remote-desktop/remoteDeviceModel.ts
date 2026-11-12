import {
  sortLanPeerEndpoints,
  type LanPairedPeer,
  type LanPeerEndpoint,
  type RemoteTarget,
  type RemoteV3DeviceSummary,
} from "@mode/shared";

/**
 * 远程控制设备页的投影（docs/specs/remote-control-unified-pairing.md R2）：
 * 页面只展示同账号 v3 Host，不再有 LAN 已配对 / 发现 / Mode Agent 三类设备。
 * 这里只做排序与 LAN 记录转换，不持有会话状态；会话仍归 registry / remote desktop controller。
 */

/** 在线优先，其次按名称（同名再按 deviceId）稳定排序；不修改入参。 */
export function sortV3Hosts(hosts: readonly RemoteV3DeviceSummary[]): RemoteV3DeviceSummary[] {
  return [...hosts].sort((left, right) => {
    if (left.online !== right.online) return left.online ? -1 : 1;
    const byName = left.name.localeCompare(right.name);
    return byName !== 0 ? byName : left.deviceId.localeCompare(right.deviceId);
  });
}

function lanPeerEndpoints(peer: LanPairedPeer): LanPeerEndpoint[] {
  if (peer.endpoints?.length) {
    return sortLanPeerEndpoints(peer.endpoints);
  }
  return peer.host && peer.port > 0
    ? [{ scheme: "http", host: peer.host, port: peer.port, kind: "lan" }]
    : [];
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
