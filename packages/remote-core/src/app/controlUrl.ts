/**
 * 服务地址 → Control WS 地址。
 *
 * 保留服务地址的路径前缀（fnOS 网关 / 反向代理子路径，如 `/app/mode`），
 * 只追加 `/ws/v1/connect`；query/hash 丢弃。
 */
export function toControlUrl(serverUrl: string): string {
  const url = new URL(serverUrl);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  url.pathname = `${url.pathname.replace(/\/+$/u, "")}/ws/v1/connect`;
  url.search = "";
  url.hash = "";
  return url.toString();
}
