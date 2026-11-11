import type { BusinessFrame, RemoteConnection } from "@mode/remote-core";

import { requestLocalHttp } from "./httpBridge.js";
import type { LocalApiTarget } from "./wsTunnel.js";

export interface RemoteV3RpcContext {
  deviceId: string;
  name: string;
  platform: string;
  version?: string;
  fingerprint: string;
  listWorkspaces?: () => Promise<Array<{ path: string; name?: string }>>;
  getLocalApi?: () => Promise<LocalApiTarget | null>;
}

/** Noise 通道内的 Host 业务 allowlist；未列出的方法一律 METHOD_NOT_FOUND。 */
export async function handleRemoteV3Frame(
  connection: RemoteConnection,
  frame: BusinessFrame,
  context: RemoteV3RpcContext,
): Promise<void> {
  if (frame.type === "ping") {
    connection.send({
      v: 1,
      type: "pong",
      ...(frame.nonce ? { nonce: frame.nonce } : {}),
    });
    return;
  }
  if (frame.type !== "rpc.request") return;
  try {
    switch (frame.method) {
      case "remote.ping":
        reply(connection, frame.id, { pong: true, now: Date.now() });
        return;
      case "remote.echo":
        reply(connection, frame.id, frame.params);
        return;
      case "remote.host.info":
        reply(connection, frame.id, {
          deviceId: context.deviceId,
          name: context.name,
          platform: context.platform,
          version: context.version ?? null,
          fingerprint: context.fingerprint,
        });
        return;
      case "remote.workspace.list": {
        if (!context.listWorkspaces) {
          fail(connection, frame.id, "METHOD_NOT_FOUND", "Host 尚未提供工作区列表");
          return;
        }
        reply(connection, frame.id, { items: await context.listWorkspaces() });
        return;
      }
      case "remote.http.request": {
        const method = frame.params.method;
        const path = frame.params.path;
        if (typeof method !== "string" || typeof path !== "string") {
          fail(connection, frame.id, "INVALID_PARAMS", "method/path 必须是字符串");
          return;
        }
        const target = context.getLocalApi ? await context.getLocalApi() : null;
        if (!target) {
          fail(connection, frame.id, "HOST_LOCAL_API_UNAVAILABLE", "Host 本机 LAN 服务未启用");
          return;
        }
        reply(
          connection,
          frame.id,
          await requestLocalHttp(target, {
            method: method as "GET" | "POST" | "PUT" | "PATCH" | "DELETE",
            path,
            headers: readHeaders(frame.params.headers),
            ...(typeof frame.params.bodyBase64 === "string"
              ? { bodyBase64: frame.params.bodyBase64 }
              : {}),
          }),
        );
        return;
      }
      default:
        fail(connection, frame.id, "METHOD_NOT_FOUND", `未知方法 ${frame.method}`);
    }
  } catch (error) {
    fail(
      connection,
      frame.id,
      "INTERNAL",
      error instanceof Error ? error.message : String(error),
    );
  }
}

function reply(connection: RemoteConnection, id: string, result: unknown): void {
  connection.send({ v: 1, type: "rpc.response", id, result });
}

function fail(connection: RemoteConnection, id: string, code: string, message: string): void {
  connection.send({ v: 1, type: "rpc.error", id, code, message, retryable: false });
}

function readHeaders(value: unknown): Record<string, string> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  const headers: Record<string, string> = {};
  for (const [key, entry] of Object.entries(value)) {
    if (typeof entry === "string") headers[key] = entry;
  }
  return headers;
}
