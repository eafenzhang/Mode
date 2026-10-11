/**
 * 被控会话请求的准入判定（纯函数，单独成文件是为了让单测不必拉起 host 的完整依赖图）。
 *
 * 口径（docs/specs/lan-remote-desktop.md 产品规则 1/5、事件顺序 ②）：
 * - 打包层 kill-switch 命中 → disabled（压死整链，只能关不能开）；
 * - 被控总开关关 → disabled（不弹窗、不签发任何票据）；
 * - 其余进入同意流（「已记住」的设备由 main 读授权列表后自动放行，不在这一层判定）。
 */
import type { RemoteDesktopErrorCode } from "@mode/shared/remote-desktop";

export function evaluateRemoteDesktopRequest(params: {
  killSwitchEngaged: boolean;
  serverEnabled: boolean;
}): { action: "reject"; error: RemoteDesktopErrorCode } | { action: "consent" } {
  if (params.killSwitchEngaged) {
    return { action: "reject", error: "disabled" };
  }
  if (!params.serverEnabled) {
    return { action: "reject", error: "disabled" };
  }
  return { action: "consent" };
}
