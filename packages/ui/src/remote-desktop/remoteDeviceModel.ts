import type { RemoteV3DeviceSummary } from "@mode/shared";

/**
 * 远程控制设备页的投影（docs/specs/remote-control-unified-pairing.md R2）：
 * 页面只展示同账号 v3 Host，不再有 LAN 已配对 / 发现 / Mode Agent 三类设备。
 * 这里只做排序，不持有会话状态；会话仍归 registry / remote desktop controller。
 */

/** 在线优先，其次按名称（同名再按 deviceId）稳定排序；不修改入参。 */
export function sortV3Hosts(hosts: readonly RemoteV3DeviceSummary[]): RemoteV3DeviceSummary[] {
  return [...hosts].sort((left, right) => {
    if (left.online !== right.online) return left.online ? -1 : 1;
    const byName = left.name.localeCompare(right.name);
    return byName !== 0 ? byName : left.deviceId.localeCompare(right.deviceId);
  });
}
