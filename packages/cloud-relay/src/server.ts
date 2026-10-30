/**
 * WSS 中继服务（docs/specs/wss-relay-mvp.md）。
 *
 * - 设备侧：`GET /_tunnel?tunnelId=…&token=…` 出站 WSS，注册 tunnelId。
 * - 主控侧：`ANY /d/<tunnelId>/...` 与对应 WS 升级，透明转发到设备隧道。
 * - 只持有连接期内存映射（TunnelRegistry），不保存任务/会话/令牌。
 */
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import type { Duplex } from "node:stream";
import { WebSocket, WebSocketServer } from "ws";

import { handleControlRequest } from "./controlApi.js";
import { startCloudRuntime, type RelayCloudOptions } from "./cloudRuntime.js";
import { handleRelayHttp } from "./httpProxy.js";
import {
  allocateStreamId,
  normalizeHeaders,
  safeEqual,
  toBuffer,
  writeUpgradeError,
} from "./internal.js";
import { isPortalPath, servePortal } from "./portal.js";
import {
  encodeTunnelDataFrame,
  parsePublicPath,
  RELAY_HTTP_TIMEOUT_MS,
  RELAY_MAX_HTTP_BODY_BYTES,
  RELAY_MAX_WS_FRAME_BYTES,
  RELAY_TUNNEL_ID_PATTERN,
  RELAY_TUNNEL_PATH,
  RELAY_WS_OPEN_TIMEOUT_MS,
  type RelayWsErrorControl,
  type RelayWsReadyControl,
} from "./protocol.js";
import { TunnelRegistry } from "./tunnelRegistry.js";

export interface RelayServerOptions {
  host?: string;
  port: number;
  token: string;
  publicBaseUrl: string;
  maxHttpBodyBytes?: number;
  httpTimeoutMs?: number;
  wsOpenTimeoutMs?: number;
  heartbeatMs?: number;
  log?: (line: string) => void;
  /**
   * Mode Cloud 控制面：提供 enrollToken 时启用 `/v1/*` 与 SQLite 设备目录
   * （docs/specs/cloud-personal-accounts.md）。
   */
  cloud?: RelayCloudOptions;
}

export interface RelayServer {
  readonly port: number;
  readonly publicBaseUrl: string;
  getTunnelIds(): string[];
  close(): Promise<void>;
}

export async function startRelayServer(options: RelayServerOptions): Promise<RelayServer> {
  const token = options.token.trim();
  if (!token) {
    throw new Error("relay token 不能为空");
  }
  const log = options.log ?? ((line: string) => console.log(line));
  const maxBody = options.maxHttpBodyBytes ?? RELAY_MAX_HTTP_BODY_BYTES;
  const httpTimeout = options.httpTimeoutMs ?? RELAY_HTTP_TIMEOUT_MS;
  const wsOpenTimeout = options.wsOpenTimeoutMs ?? RELAY_WS_OPEN_TIMEOUT_MS;
  const registry = new TunnelRegistry(log, options.heartbeatMs);
  const wss = new WebSocketServer({ noServer: true });

  const cloudRuntime = options.cloud
    ? startCloudRuntime({
        cloud: options.cloud,
        relayToken: token,
        publicBaseUrl: options.publicBaseUrl,
        getTunnelIds: () => registry.ids(),
        closeTunnel: (tunnelId) => {
          // revoke 设备：主动断开在线隧道；令牌已删，重连会被 401 拒绝。
          registry.close(tunnelId, 1000, "revoked");
        },
        log,
      })
    : null;
  const control = cloudRuntime?.control ?? null;

  const handleTunnelUpgrade = (
    req: IncomingMessage,
    socket: Duplex,
    head: Buffer,
    url: URL,
  ): void => {
    const tunnelId = url.searchParams.get("tunnelId")?.trim() ?? "";
    const presented = url.searchParams.get("token")?.trim() ?? "";
    if (!RELAY_TUNNEL_ID_PATTERN.test(tunnelId)) {
      writeUpgradeError(socket, 401, "Unauthorized");
      return;
    }
    // 每设备令牌优先（docs/specs/cloud-personal-accounts.md）；共享 secret 回退
    // （纯中继部署 / 运维过渡路径）。
    const perDeviceOk = control ? control.verifyTunnelToken(tunnelId, presented) : false;
    if (!perDeviceOk && !safeEqual(token, presented)) {
      writeUpgradeError(socket, 401, "Unauthorized");
      return;
    }
    wss.handleUpgrade(req, socket, head, (ws) => registry.register(ws, tunnelId));
  };

  const handleHttp = async (req: IncomingMessage, res: ServerResponse): Promise<void> => {
    const url = new URL(req.url ?? "/", "http://relay.local");
    if (url.pathname === "/_health") {
      res.writeHead(200, { "content-type": "text/plain" });
      res.end("ok");
      return;
    }
    if (isPortalPath(url.pathname)) {
      // 门户只在启用控制面时提供；没有账号服务时保持 404，不误导访问者。
      if (!control) {
        res.writeHead(404);
        res.end();
        return;
      }
      servePortal(res);
      return;
    }
    if (url.pathname.startsWith("/v1/")) {
      if (!control) {
        res.writeHead(503, { "content-type": "application/json; charset=utf-8" });
        res.end(JSON.stringify({ error: "control_disabled" }));
        return;
      }
      await handleControlRequest(req, res, url, control);
      return;
    }
    const match = parsePublicPath(url.pathname);
    if (!match) {
      res.writeHead(404);
      res.end();
      return;
    }
    const tunnel = registry.get(match.tunnelId);
    if (!tunnel) {
      res.writeHead(503);
      res.end("relay tunnel offline");
      return;
    }
    await handleRelayHttp({
      req,
      res,
      tunnel,
      targetPath: `${match.rest}${url.search}`,
      maxBody,
      timeoutMs: httpTimeout,
    });
  };

  const handleControllerUpgrade = async (
    req: IncomingMessage,
    socket: Duplex,
    head: Buffer,
    url: URL,
    match: { tunnelId: string; rest: string },
  ): Promise<void> => {
    const tunnel = registry.get(match.tunnelId);
    if (!tunnel || tunnel.ws.readyState !== WebSocket.OPEN) {
      writeUpgradeError(socket, 503, "Service Unavailable");
      return;
    }
    const streamId = allocateStreamId(tunnel);
    const control = {
      type: "ws-open" as const,
      streamId,
      path: `${match.rest}${url.search}`,
      headers: normalizeHeaders(req.headers),
    };
    let opened: RelayWsReadyControl | RelayWsErrorControl;
    try {
      opened = await new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          tunnel.pendingWs.delete(streamId);
          reject(new Error("ws open timeout"));
        }, wsOpenTimeout);
        tunnel.pendingWs.set(streamId, {
          resolve: (message) => resolve(message),
          timer,
        });
        tunnel.ws.send(JSON.stringify(control), (error) => {
          if (error) {
            clearTimeout(timer);
            tunnel.pendingWs.delete(streamId);
            reject(error);
          }
        });
      });
    } catch {
      writeUpgradeError(socket, 504, "Gateway Timeout");
      return;
    }
    if (opened.type === "ws-error") {
      writeUpgradeError(socket, 502, "Bad Gateway");
      return;
    }
    wss.handleUpgrade(req, socket, head, (controllerWs) => {
      tunnel.activeWs.set(streamId, controllerWs);
      controllerWs.on("message", (data, isBinary) => {
        if (tunnel.ws.readyState !== WebSocket.OPEN) {
          return;
        }
        if (tunnel.ws.bufferedAmount > RELAY_MAX_WS_FRAME_BYTES * 2) {
          controllerWs.close(1013, "relay backpressure");
          return;
        }
        tunnel.ws.send(encodeTunnelDataFrame(streamId, isBinary, toBuffer(data)));
      });
      controllerWs.on("close", (code, reason) => {
        tunnel.activeWs.delete(streamId);
        if (tunnel.ws.readyState === WebSocket.OPEN) {
          tunnel.ws.send(
            JSON.stringify({
              type: "ws-close",
              streamId,
              code,
              reason: reason.toString(),
            }),
          );
        }
      });
      controllerWs.on("error", () => {
        tunnel.activeWs.delete(streamId);
      });
    });
  };

  const server = createServer((req, res) => {
    void handleHttp(req, res).catch(() => {
      if (!res.headersSent) {
        res.writeHead(500);
      }
      res.end();
    });
  });
  server.on("upgrade", (req, socket, head) => {
    const url = new URL(req.url ?? "/", "http://relay.local");
    if (url.pathname === RELAY_TUNNEL_PATH) {
      handleTunnelUpgrade(req, socket, head, url);
      return;
    }
    const match = parsePublicPath(url.pathname);
    if (!match) {
      writeUpgradeError(socket, 404, "Not Found");
      return;
    }
    void handleControllerUpgrade(req, socket, head, url, match).catch(() => {
      writeUpgradeError(socket, 502, "Bad Gateway");
    });
  });

  await new Promise<void>((resolve, reject) => {
    const onError = (error: Error) => reject(error);
    server.once("error", onError);
    server.listen(options.port, options.host ?? "127.0.0.1", () => {
      server.off("error", onError);
      resolve();
    });
  });
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : options.port;

  return {
    port,
    publicBaseUrl: options.publicBaseUrl,
    getTunnelIds: () => registry.ids(),
    async close() {
      cloudRuntime?.close();
      registry.closeAll(1001, "relay shutting down");
      await new Promise<void>((resolve) => {
        wss.close(() => resolve());
        server.close(() => resolve());
        (server as unknown as { closeAllConnections?: () => void }).closeAllConnections?.();
      });
    },
  };
}
