/**
 * 被控端「共享为可远控设备」提示性开关 + 系统 RDP 状态探测的装配
 * （docs/specs/remote-desktop-system-rdp.md 产品规则 4/5）。
 *
 * 开关**不是安全闸门**：系统 RDP 监听 3389，本应用拦不住任何连接。它只决定本机是否作为
 * 可远控设备出现在对端设备页（经 server-info 的 remoteDesktop 能力位）。
 * 存储键沿用改造前的 `remote-desktop:server-enabled`，升级后不丢设置。
 */
import type { ICredentialService } from "@mode/services";
import { createSystemRdpProbe, runRegQuery, type SystemRdpProbe } from "./systemRdpState.js";

export const REMOTE_DESKTOP_SHARING_CREDENTIAL_KEY = "remote-desktop:server-enabled";

export async function readRemoteDesktopSharing(credentials: ICredentialService): Promise<boolean> {
  return (await credentials.load(REMOTE_DESKTOP_SHARING_CREDENTIAL_KEY)) === "1";
}

export async function writeRemoteDesktopSharing(
  credentials: ICredentialService,
  enabled: boolean,
): Promise<void> {
  if (enabled) {
    await credentials.save(REMOTE_DESKTOP_SHARING_CREDENTIAL_KEY, "1");
  } else {
    await credentials.delete(REMOTE_DESKTOP_SHARING_CREDENTIAL_KEY);
  }
}

/**
 * 建系统 RDP 探测（非 win32 返回 null：该能力位只在 win32 被控端有意义）。
 * `warn` 用于探测失败与版本号无法判定的留痕。
 */
export function createHostSystemRdpProbe(
  platform: string,
  warn: (message: string) => void,
): SystemRdpProbe | null {
  if (platform !== "win32") {
    return null;
  }
  return createSystemRdpProbe({ runRegQuery, warn });
}
