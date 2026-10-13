/**
 * 「远程桌面」设备页的可用性判定（纯函数，单测见 packages/ui/test/remoteDesktopDeviceAvailability.test.ts）。
 *
 * 规则（docs/specs/lan-remote-desktop.md 产品规则 1/5 与降级语义；
 * 原生媒体面版本门见 docs/specs/lan-remote-desktop-native-media.md）：
 * - 能力位 `remoteDesktop`：win32 && 被控开关 && kill-switch；旧对端缺键 = false → 不支持；
 * - 版本门 `remoteDesktopNative`（纯直替）：旧对端缺 native 键 = false → 不支持（同一 unsupported 文案）；
 * - 在线状态：宿主探测失败一律按离线展示（能力位不猜测），由用户手动重试；
 * - 不可连接时入口置灰并给稳定文案，不建回退通道、不自动重试。
 */
import type { RemoteDesktopPeerCapability } from "./platformTypes.js";

/** 单个设备的探测状态（设备页内逐设备维护；loading/error 都来自宿主往返）。 */
export type RemoteDesktopDeviceProbe =
  | { status: "probing" }
  | { status: "ok"; capability: RemoteDesktopPeerCapability }
  | { status: "probe-failed" };

export type RemoteDesktopDeviceBlockReason =
  | "probing"
  | "probe-failed"
  | "offline"
  | "unsupported"
  /** 本机被控中（spec 产品规则 9）：整卡置灰，断开被控后刷新恢复。 */
  | "locally-controlled";

export interface RemoteDesktopDeviceAvailability {
  canConnect: boolean;
  /** 不能连接的原因；canConnect 为真时为 null。 */
  reason: RemoteDesktopDeviceBlockReason | null;
}

export function resolveRemoteDesktopDeviceAvailability(
  probe: RemoteDesktopDeviceProbe | undefined,
  options: { locallyControlled?: boolean } = {},
): RemoteDesktopDeviceAvailability {
  // 本地被控状态优先：无论对端探测结果如何，本机被控中一律不可发起。
  if (options.locallyControlled) {
    return { canConnect: false, reason: "locally-controlled" };
  }
  if (!probe || probe.status === "probing") {
    return { canConnect: false, reason: "probing" };
  }
  if (probe.status === "probe-failed") {
    return { canConnect: false, reason: "probe-failed" };
  }
  if (!probe.capability.online) {
    return { canConnect: false, reason: "offline" };
  }
  // 版本门（纯直替）：缺 native 键 = 旧对端，不进会话；复用既有 unsupported 文案
  //（已含「或版本过旧」），不新增 reason/key。
  if (!probe.capability.remoteDesktop || !probe.capability.remoteDesktopNative) {
    return { canConnect: false, reason: "unsupported" };
  }
  return { canConnect: true, reason: null };
}

/** 设备展示名：优先平台/设备自报名，其次回退 serverId（与配对列表口径一致）。 */
export function resolveRemoteDesktopDeviceLabel(peer: { serverId: string; name?: string }): string {
  return peer.name?.trim() || peer.serverId;
}

/**
 * 「检测设备」列表过滤（spec: docs/specs/lan-remote-desktop.md UI 规则「设备页」）：
 * 已配对 serverId 从检测结果中过滤——配对成功立即消失，重新检测也不再出现；
 * 全部被过滤时 hiddenCount 供调用方给「检测到的设备均已配对」提示。
 */
export function filterUnpairedDiscoveredPeers<T extends { serverId: string }>(
  peers: readonly T[],
  pairedServerIds: readonly string[],
): { visible: T[]; hiddenCount: number } {
  const paired = new Set(pairedServerIds);
  const visible = peers.filter((peer) => !paired.has(peer.serverId));
  return { visible, hiddenCount: peers.length - visible.length };
}
