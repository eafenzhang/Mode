import type { LanAccessState } from "@mode/shared";

/**
 * Web/手机远控入口地址（纯函数，单测见 packages/ui/test/webRemoteEntry.test.ts）。
 *
 * 优先使用 Host 回报的第一个监听地址（局域网/Tailscale）；地址列表为空时才回退
 * `127.0.0.1:<port>`，让用户在未开启 LAN 时也能看到本机调试地址。不要在 UI 里
 * 第二遍拼地址——LAN 状态的唯一所有者仍是 Host。
 */
export function resolveWebRemoteAddress(state: LanAccessState | null | undefined): string {
  const address = state?.addresses.find((candidate) => candidate.trim().length > 0)?.trim();
  if (address) {
    return address;
  }
  return state?.port ? `127.0.0.1:${state.port}` : "";
}

export function resolveWebRemoteUrl(state: LanAccessState | null | undefined): string {
  const address = resolveWebRemoteAddress(state);
  return address ? `http://${address}/` : "";
}

/**
 * 在线 web/手机远控会话数：旧 Host 不下发该键，按 0 处理；异常值同样收敛到 0，
 * 避免弹层把负值/NaN 当成「已连接」。
 */
export function resolveActiveWebSessions(state: LanAccessState | null | undefined): number {
  const count = state?.activeWebSessions;
  return typeof count === "number" && Number.isFinite(count) && count > 0
    ? Math.floor(count)
    : 0;
}
