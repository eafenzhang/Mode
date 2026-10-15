/**
 * 「远程桌面」设备页的可用性判定（纯函数，单测见 packages/ui/test/remoteDesktopDeviceAvailability.test.ts）。
 *
 * 规格：docs/specs/remote-desktop-system-rdp.md（产品规则 7/8、失败语义表）。
 *
 * 判据优先级（自上而下，命中即返回）：
 *   1. 探测中 / 探测失败 → 置灰（探测未完成不猜测能力位）；
 *   2. 对端离线（`online === false`）→ 置灰；
 *   3. `remoteDesktop === false`（对端未共享为可远控设备）→ 置灰；
 *   4. `systemRdp.editionOk === false`（家庭版）→ 置灰（系统 RDP 主机不可用）；
 *   5. `systemRdp.enabled === false`（未开启）→ 置灰（附开启指引）；
 *   6. 其余 → 可连接。
 *
 * 为什么 `remoteDesktop` 参与置灰：它是**对端机主的共享意愿声明**（提示性开关 + kill-switch）。
 * 应用拦不住 3389，但这不代表主控该无视对方的意愿去连——若该位不参与，被控端的共享开关
 * 在 UI 上就完全等于不存在（假开关）。注意该位为 false 有多个来源（开关关 / kill-switch /
 * 旧对端开关关），文案只陈述「对端未共享」，不猜具体是哪一个。
 *
 * 为什么 `systemRdp` 缺键时不置灰：旧对端（未升级）或注册表探测未就绪时该键不存在，
 * 这代表「状态未知」而非「不可用」。若据此置灰，会在升级/探测窗口期把可用设备锁死；
 * 真正不可用时 mstsc 自身会给出失败（spec「失败语义」表：缺键 → 不置灰，按状态未知呈现）。
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
  /** 对端未共享为可远控设备（提示性开关关 / kill-switch / 旧对端开关关）。 */
  | "not-shared"
  /** 对端为 Windows 家庭版，系统远程桌面不可用（spec 产品规则 8）。 */
  | "system-rdp-home"
  /** 对端未开启系统远程桌面（spec 产品规则 4/8）。 */
  | "system-rdp-disabled";

export interface RemoteDesktopDeviceAvailability {
  canConnect: boolean;
  /** 不能连接的原因；canConnect 为真时为 null。 */
  reason: RemoteDesktopDeviceBlockReason | null;
}

export function resolveRemoteDesktopDeviceAvailability(
  probe: RemoteDesktopDeviceProbe | undefined,
): RemoteDesktopDeviceAvailability {
  if (!probe || probe.status === "probing") {
    return { canConnect: false, reason: "probing" };
  }
  if (probe.status === "probe-failed") {
    return { canConnect: false, reason: "probe-failed" };
  }
  const capability = probe.capability;
  if (!capability.online) {
    return { canConnect: false, reason: "offline" };
  }
  // 对端机主未共享：先于系统能力检查——对方没打算被远控时，讨论能不能连没有意义。
  if (capability.remoteDesktop !== true) {
    return { canConnect: false, reason: "not-shared" };
  }
  // 家庭版优先于「未开启」：家庭版即使用户想开也开不了，指引要指向真实根因。
  if (capability.systemRdp?.editionOk === false) {
    return { canConnect: false, reason: "system-rdp-home" };
  }
  if (capability.systemRdp?.enabled === false) {
    return { canConnect: false, reason: "system-rdp-disabled" };
  }
  return { canConnect: true, reason: null };
}

/** 设备展示名：优先平台/设备自报名，其次回退 serverId（与配对列表口径一致）。 */
export function resolveRemoteDesktopDeviceLabel(peer: { serverId: string; name?: string }): string {
  return peer.name?.trim() || peer.serverId;
}

/**
 * 设备卡状态胶囊的语义投影：胶囊表达状态、连接入口表达可操作性，两者同源
 * （任一探测下「就绪档」⇔ `canConnect`，由单测锁定）。只给语义与 i18n key，
 * 色调类名留在展示层，因此这里可以纯函数单测。
 */
export type RemoteDesktopDeviceStatusTone = "ready" | "attention" | "muted";

export function resolveRemoteDesktopDeviceStatusPill(probe: RemoteDesktopDeviceProbe | undefined): {
  labelId: string;
  tone: RemoteDesktopDeviceStatusTone;
} {
  if (!probe || probe.status === "probing") {
    return { labelId: "remoteDesktop.device.status.probing", tone: "muted" };
  }
  if (probe.status === "probe-failed") {
    return { labelId: "remoteDesktop.device.status.unknown", tone: "muted" };
  }
  const availability = resolveRemoteDesktopDeviceAvailability(probe);
  switch (availability.reason) {
    case "offline":
      return { labelId: "remoteDesktop.device.status.offline", tone: "attention" };
    case "not-shared":
      return { labelId: "remoteDesktop.device.status.notShared", tone: "attention" };
    case "system-rdp-home":
      return { labelId: "remoteDesktop.device.status.homeEdition", tone: "attention" };
    case "system-rdp-disabled":
      return { labelId: "remoteDesktop.device.status.rdpDisabled", tone: "attention" };
    default:
      return { labelId: "remoteDesktop.device.status.ready", tone: "ready" };
  }
}

/**
 * 「检测设备」列表过滤（spec: docs/specs/remote-desktop-system-rdp.md 产品规则 1）：
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
