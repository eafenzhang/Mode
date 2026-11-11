/** 服务地址 → Control WS 地址；只保留 origin，忽略 REST 路径/query。 */
export function toControlUrl(serverUrl: string): string {
  const url = new URL(serverUrl);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  url.pathname = "/ws/v1/connect";
  url.search = "";
  url.hash = "";
  return url.toString();
}
