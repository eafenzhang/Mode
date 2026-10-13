import type { RemoteTarget } from "@mode/shared";
import { isRemoteWorkspaceIdentity, parseRemoteWorkspaceIdentity } from "@mode/shared";
import type { CuaPermissionStatus, CuaPermissionStatusResult } from "@mode/services";

export type ComputerUseAvailabilityKind =
  | "local-macos"
  | "local-windows"
  | "local-linux"
  | "remote-ssh"
  | "remote-wsl"
  | "remote-docker"
  | "remote-lan"
  | "remote-server"
  | "web";

export interface ComputerUseAvailability {
  kind: ComputerUseAvailabilityKind;
  supported: boolean;
}

export interface ComputerUseAvailabilityInput {
  isDesktop?: boolean;
  isMacDesktop?: boolean;
  isWindowsDesktop?: boolean;
  remoteSessionId?: string | null;
  remoteTarget?: RemoteTarget | null;
  workspaceIdentity?: string | null;
  /** LAN 对端握手声明的 computerUse 能力位；缺省（旧对端/非 LAN）按不支持处理。 */
  lanComputerUseCapable?: boolean;
}

export function resolveComputerUseAvailability({
  isDesktop = false,
  isMacDesktop = false,
  isWindowsDesktop = false,
  remoteSessionId,
  remoteTarget,
  workspaceIdentity,
  lanComputerUseCapable,
}: ComputerUseAvailabilityInput = {}): ComputerUseAvailability {
  const isRemote = Boolean(
    remoteSessionId ||
    remoteTarget ||
    (workspaceIdentity?.trim() && isRemoteWorkspaceIdentity(workspaceIdentity.trim())),
  );
  if (isRemote) {
    // LAN：完整 desktop 对端，支持与否由握手能力位决定（docs/specs/lan-remote-computer-use.md）。
    // 仅按 identity kind 判定，能力位缺省/为 false 时维持 unsupported。
    const identityKind = workspaceIdentity?.trim()
      ? parseRemoteWorkspaceIdentity(workspaceIdentity.trim())?.kind
      : undefined;
    if (remoteTarget?.kind === "lan" || identityKind === "lan") {
      return { kind: "remote-lan", supported: lanComputerUseCapable === true };
    }
    const remoteKind: ComputerUseAvailabilityKind =
      remoteTarget?.kind === "ssh"
        ? "remote-ssh"
        : remoteTarget?.kind === "wsl"
          ? "remote-wsl"
          : remoteTarget?.kind === "docker"
            ? "remote-docker"
            : "remote-server";
    return {
      kind: remoteKind,
      supported: false,
    };
  }
  if (!isDesktop) return { kind: "web", supported: false };
  if (isMacDesktop) return { kind: "local-macos", supported: true };
  if (isWindowsDesktop) return { kind: "local-windows", supported: true };
  return { kind: "local-linux", supported: false };
}

/**
 * 远端（LAN 对端）pane 权限行的可见判定（docs/specs/lan-remote-computer-use.md）：
 * `status !== null && status.available !== false`——对端 getStatus 的成功回包**不携带**
 * `available` 字段（成功形态见 packages/services/src/node.ts），属可用状态；显式
 * `available: false` 是对端 win/linux/插件未启用的不可用回包；尚未查询到（null）同样隐藏。
 * 本机 pane 不走该判定（维持既有 isCuaPermissionStatusAvailable 谓词与平台门不动）。
 * 该判定为真时 status 必属 CuaPermissionStatus（Unavailable 恒带 available:false，
 * null 已排除），故声明为类型收窄，供行内容（accessibility/screenRecording）取值。
 */
export function isRemoteCuaPermissionRowVisible(
  status: CuaPermissionStatusResult | null,
): status is CuaPermissionStatus {
  return status !== null && status.available !== false;
}

const COMPUTER_USE_SEARCH_TERMS = ["电脑控制", "computer use", "mode-cua", "cua"];

export function matchesComputerUseSearch(query: string): boolean {
  const normalized = query.trim().toLocaleLowerCase();
  if (!normalized) return true;
  return COMPUTER_USE_SEARCH_TERMS.some((term) => term.includes(normalized));
}

export function isComputerUseRemoteOrLinux(availability: ComputerUseAvailability): boolean {
  return !availability.supported && availability.kind !== "web";
}
