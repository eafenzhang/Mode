// fnOS 统一网关接入代理（随 fpk 分发，在 NAS 上用捆绑 Node 运行）。
//
// 职责（契约见 docs/specs/fnos-fpk-packaging.md「统一网关接入」）：
//  1. 监听 target 卷内的 unix socket（$TRIM_APPDEST/app.sock），只服务 fnOS 网关转发的流量；
//  2. 前缀剥离：网关可能带 /app/mode 前缀转发（也兼容已剥离的请求），统一还原为根路径；
//  3. 令牌注入：网关流量已由 fnOS 校验登录态，这里补上访问令牌 Cookie，
//     桌面入口无需在 URL 里携带令牌；LAN 直连 3030 端口仍由服务端严格校验令牌。
//
// 环境变量：MODE_FNOS_SOCKET（unix socket 路径）
//   MODE_FNOS_TARGET_PORT（转发目标端口，默认 3030）
//   MODE_FNOS_PREFIX（网关前缀，默认 /app/mode）
//   MODE_FNOS_TOKEN_FILE（令牌文件路径）
//   MODE_FNOS_LISTEN_PORT（本地测试用 TCP 回退：走 127.0.0.1 端口而不是 unix socket）

import { createServer as createHttpServer, request as httpRequest } from "node:http";
import { connect } from "node:net";
import { readFileSync } from "node:fs";
import { unlinkSync } from "node:fs";

const socketPath = process.env.MODE_FNOS_SOCKET?.trim() || "";
const listenPort = Number(process.env.MODE_FNOS_LISTEN_PORT || "0");
const targetPort = Number(process.env.MODE_FNOS_TARGET_PORT || "3030");
const prefix = (process.env.MODE_FNOS_PREFIX?.trim() || "/app/mode").replace(/\/+$/u, "");
const tokenFile = process.env.MODE_FNOS_TOKEN_FILE?.trim() || "";
const COOKIE_NAME = "tutto_lite_token";

function loadToken() {
  try {
    return readFileSync(tokenFile, "utf8").trim();
  } catch {
    return "";
  }
}

const token = loadToken();
if (!token) {
  console.error(`[proxy] 令牌不可读（${tokenFile || "未配置"}）：网关流量将不带令牌，可能 401`);
}

function rewritePath(url) {
  if (url === prefix) return "/";
  if (url.startsWith(`${prefix}/`)) return url.slice(prefix.length);
  if (url.startsWith(`${prefix}?`)) return `/${url.slice(prefix.length)}`;
  return url;
}

function withTokenCookie(headers) {
  if (!token) return headers;
  const existing = headers.cookie;
  return {
    ...headers,
    cookie: existing ? `${existing}; ${COOKIE_NAME}=${token}` : `${COOKIE_NAME}=${token}`,
  };
}

const server = createHttpServer((req, res) => {
  const upstream = httpRequest(
    {
      host: "127.0.0.1",
      port: targetPort,
      method: req.method,
      path: rewritePath(req.url ?? "/"),
      headers: withTokenCookie({ ...req.headers }),
    },
    (upstreamRes) => {
      res.writeHead(upstreamRes.statusCode ?? 502, upstreamRes.headers);
      upstreamRes.pipe(res);
    },
  );
  upstream.on("error", (error) => {
    console.error(`[proxy] 上游请求失败：${error.message}`);
    if (!res.headersSent) res.writeHead(502);
    res.end();
  });
  req.pipe(upstream);
});

// WebSocket 升级：手工重写请求行（http.request 不承载 upgrade），双向透传字节流。
server.on("upgrade", (req, socket, head) => {
  const upstream = connect(targetPort, "127.0.0.1", () => {
    const headers = withTokenCookie({ ...req.headers });
    const lines = [`${req.method} ${rewritePath(req.url ?? "/")} HTTP/1.1`];
    for (const [name, value] of Object.entries(headers)) {
      if (value === undefined) continue;
      for (const one of Array.isArray(value) ? value : [value]) {
        lines.push(`${name}: ${one}`);
      }
    }
    upstream.write(`${lines.join("\r\n")}\r\n\r\n`);
    if (head?.length) upstream.write(head);
    socket.pipe(upstream);
    upstream.pipe(socket);
  });
  upstream.on("error", (error) => {
    console.error(`[proxy] 上游升级失败：${error.message}`);
    socket.destroy();
  });
  socket.on("error", () => upstream.destroy());
});

function cleanupSocket() {
  if (!socketPath) return;
  try {
    unlinkSync(socketPath);
  } catch {
    // 不存在即视为已清理。
  }
}

function shutdown() {
  cleanupSocket();
  process.exit(0);
}
process.on("SIGTERM", shutdown);
process.on("SIGINT", shutdown);

if (listenPort > 0) {
  server.listen(listenPort, "127.0.0.1", () => {
    console.log(`[proxy] listening on 127.0.0.1:${listenPort} -> 127.0.0.1:${targetPort} prefix=${prefix}`);
  });
} else {
  if (!socketPath) {
    console.error("[proxy] 缺少 MODE_FNOS_SOCKET 且未设置 MODE_FNOS_LISTEN_PORT");
    process.exit(1);
  }
  // 上次退出可能残留 socket 文件；先清理再监听。
  cleanupSocket();
  server.listen(socketPath, () => {
    console.log(`[proxy] listening on ${socketPath} -> 127.0.0.1:${targetPort} prefix=${prefix}`);
  });
  server.on("error", (error) => {
    console.error(`[proxy] socket 监听失败：${error.message}`);
    process.exit(1);
  });
}
