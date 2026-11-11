/**
 * Relay 端点解析。
 *
 * 部署在 fnOS 网关/反向代理后面时，服务端只知道子路径（如 `/app/mode`），
 * 无法预知客户端访问域名；此时下发相对 relay 路径，由客户端按自己的服务地址
 * 拼成 `wss://<host>/<prefix>/ws/v1/relay`。绝对地址原样返回。
 */
export function resolveRelayEndpoint(endpoint: string, serverUrl: string): string {
  if (!endpoint.startsWith("/")) return endpoint;
  const base = new URL(serverUrl);
  base.protocol = base.protocol === "https:" ? "wss:" : "ws:";
  base.pathname = endpoint;
  base.search = "";
  base.hash = "";
  return base.toString();
}
