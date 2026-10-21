/** 主控 HTTP → 设备隧道的转发（docs/specs/wss-relay-mvp.md）。 */
import type { IncomingMessage, ServerResponse } from "node:http";
import { WebSocket } from "ws";

import {
  allocateStreamId,
  normalizeHeaders,
  readBody,
  writeHttpResponse,
  type TunnelConnection,
} from "./internal.js";
import type { RelayHttpErrorControl, RelayHttpResponseControl } from "./protocol.js";

export async function handleRelayHttp(params: {
  req: IncomingMessage;
  res: ServerResponse;
  tunnel: TunnelConnection;
  targetPath: string;
  maxBody: number;
  timeoutMs: number;
}): Promise<void> {
  const { req, res, tunnel } = params;
  if (tunnel.ws.readyState !== WebSocket.OPEN) {
    res.writeHead(503);
    res.end("relay tunnel offline");
    return;
  }
  let body: Buffer;
  try {
    body = await readBody(req, params.maxBody);
  } catch {
    res.writeHead(413, { connection: "close" });
    res.end("body too large");
    return;
  }
  const streamId = allocateStreamId(tunnel);
  const control = {
    type: "http-request" as const,
    streamId,
    method: req.method ?? "GET",
    path: params.targetPath,
    headers: normalizeHeaders(req.headers),
    ...(body.length > 0 ? { bodyBase64: body.toString("base64") } : {}),
  };
  let response: RelayHttpResponseControl | RelayHttpErrorControl;
  try {
    response = await new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        tunnel.pendingHttp.delete(streamId);
        reject(new Error("relay request timeout"));
      }, params.timeoutMs);
      tunnel.pendingHttp.set(streamId, {
        resolve: (message) => resolve(message),
        timer,
      });
      tunnel.ws.send(JSON.stringify(control), (error) => {
        if (error) {
          clearTimeout(timer);
          tunnel.pendingHttp.delete(streamId);
          reject(error);
        }
      });
    });
  } catch {
    res.writeHead(502);
    res.end("relay request failed");
    return;
  }
  if (response.type === "http-error") {
    res.writeHead(502);
    res.end(response.message);
    return;
  }
  writeHttpResponse(res, response);
}
