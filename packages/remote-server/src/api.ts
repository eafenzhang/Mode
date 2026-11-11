import type { IncomingMessage, ServerResponse } from "node:http";

import { parseDeviceDescriptor, type DeviceDescriptor } from "@mode/remote-core";

import {
  ApiError,
  createGithubSession,
  readJson,
  requireString,
  sendJson,
  type ApiDeps,
} from "./apiSupport.js";
import { handleGithubDeviceRoute } from "./deviceAuth.js";
import { handlePairingRoute } from "./pairingRoutes.js";
import { descriptorOf, GatewayError, parseAddresses } from "./gateway.js";
import { FixedWindowLimiter } from "./rateLimit.js";
import { StoreError, type DeviceRow } from "./store.js";

export type { ApiDeps } from "./apiSupport.js";

type Auth =
  | { kind: "web"; accountId: string; displayName?: string }
  | { kind: "device"; device: DeviceRow };

export function createApiHandler(deps: ApiDeps) {
  const general = new FixedWindowLimiter(120, 60_000);
  const auth = new FixedWindowLimiter(10, 60_000);
  const deviceAuth = new FixedWindowLimiter(60, 60_000);
  return async (req: IncomingMessage, res: ServerResponse, url: URL): Promise<void> => {
    try {
      const key = req.socket.remoteAddress ?? "unknown";
      if (!general.allow(key)) {
        throw new ApiError(429, "rate_limited", "请求过于频繁，请稍后重试");
      }
      if (url.pathname.startsWith("/api/v1/auth/")) {
        const limiter = url.pathname.startsWith("/api/v1/auth/github/device/")
          ? deviceAuth
          : auth;
        if (!limiter.allow(key)) {
          throw new ApiError(429, "rate_limited", "登录请求过于频繁，请稍后重试");
        }
      }
      await route(deps, req, res, url);
    } catch (error) {
      sendError(res, error);
    }
  };
}

async function route(
  deps: ApiDeps,
  req: IncomingMessage,
  res: ServerResponse,
  url: URL,
): Promise<void> {
  const method = req.method ?? "GET";
  const path = url.pathname;
  if (await handleGithubDeviceRoute(deps, req, res, method, path)) return;
  if (
    await handlePairingRoute(deps, req, res, method, path, (request) => requireDevice(deps, request))
  ) {
    return;
  }

  if (method === "POST" && path === "/api/v1/auth/github") {
    const body = await readJson(req);
    const accessToken = requireString(body, "accessToken");
    sendJson(
      res,
      200,
      await createGithubSession(
        deps,
        accessToken,
        typeof body.clientLabel === "string" ? body.clientLabel : undefined,
      ),
    );
    return;
  }

  if (method === "POST" && path === "/api/v1/auth/refresh") {
    const auth = requireWeb(deps, req);
    const rotated = deps.store.rotateWebSession(requireBearer(req));
    if (!rotated) throw new ApiError(401, "session_invalid", "Web 会话无效");
    sendJson(res, 200, { sessionToken: rotated.token, expiresAt: rotated.expiresAt, account: { id: auth.accountId } });
    return;
  }

  if (method === "POST" && path === "/api/v1/auth/logout") {
    requireWeb(deps, req);
    deps.store.revokeWebSession(requireBearer(req));
    sendJson(res, 200, { ok: true });
    return;
  }

  if (method === "GET" && path === "/api/v1/auth/me") {
    const auth = requireAuth(deps, req);
    if (auth.kind === "web") {
      sendJson(res, 200, { kind: "web", account: { id: auth.accountId, displayName: auth.displayName } });
    } else {
      sendJson(res, 200, { kind: "device", device: descriptorOf(auth.device) });
    }
    return;
  }

  if (method === "POST" && path === "/api/v1/devices") {
    const auth = requireWeb(deps, req);
    const body = await readJson(req);
    const descriptor = parseDeviceDescriptor(body.device, "device");
    const addresses = parseAddressList(body.addresses);
    const device = deps.store.upsertDevice({
      accountId: auth.accountId,
      descriptor,
      addresses,
    });
    const credentials = deps.store.issueDeviceCredentials(device.id);
    sendJson(res, 201, {
      accountId: device.account_id,
      device: descriptorOf(device),
      ...credentials,
    });
    return;
  }

  if (method === "POST" && path === "/api/v1/devices/register-owned-role") {
    const auth = requireDevice(deps, req);
    const body = await readJson(req);
    const descriptor = parseDeviceDescriptor(body.device, "device");
    if (descriptor.role === auth.device.role) {
      throw new ApiError(400, "same_role", "只能注册相反角色");
    }
    if (descriptor.deviceId === auth.device.id) {
      throw new ApiError(400, "same_device", "相反角色必须使用新的 deviceId");
    }
    const device = deps.store.upsertDevice({
      accountId: auth.device.account_id,
      descriptor,
    });
    const credentials = deps.store.issueDeviceCredentials(device.id);
    sendJson(res, 201, {
      accountId: device.account_id,
      device: descriptorOf(device),
      ...credentials,
    });
    return;
  }

  if (method === "POST" && path === "/api/v1/devices/refresh") {
    const body = await readJson(req);
    const deviceId = requireString(body, "deviceId");
    const refreshToken = requireString(body, "refreshToken");
    const credentials = deps.store.rotateDeviceRefresh(deviceId, refreshToken);
    sendJson(res, 200, credentials);
    return;
  }

  if (method === "GET" && path === "/api/v1/devices") {
    const device = requireDevice(deps, req).device;
    const peers = deps.store.listPeerDevices(device.id);
    sendJson(res, 200, {
      items: peers.map((peer) => ({
        device: descriptorOf(peer),
        online: deps.gateway.isOnline(peer.id),
        addresses: parseAddresses(peer.addresses),
      })),
    });
    return;
  }

  const deviceMatch = /^\/api\/v1\/devices\/([^/]+)$/.exec(path);
  if (deviceMatch) {
    const targetId = decodeURIComponent(deviceMatch[1]!);
    if (method === "GET") {
      const auth = requireDevice(deps, req);
      const target = deps.store.findDevice(targetId);
      if (!target || target.revoked_at !== null) throw new ApiError(404, "device_not_found", "设备不存在");
      if (target.id !== auth.device.id && !deps.store.hasMembership(auth.device.id, target.id)) {
        throw new ApiError(403, "membership_required", "没有访问该设备的授权");
      }
      sendJson(res, 200, {
        device: descriptorOf(target),
        online: deps.gateway.isOnline(target.id),
        addresses: parseAddresses(target.addresses),
      });
      return;
    }
    if (method === "DELETE") {
      const auth = requireAuth(deps, req);
      const target = deps.store.findDevice(targetId);
      if (!target) throw new ApiError(404, "device_not_found", "设备不存在");
      if (auth.kind === "web") {
        if (target.account_id !== auth.accountId) throw new ApiError(403, "forbidden", "无权撤销该设备");
      } else if (auth.device.id !== target.id) {
        throw new ApiError(403, "forbidden", "设备只能撤销自己");
      }
      deps.store.revokeDevice(target.id);
      deps.gateway.closeDevice(target.id);
      sendJson(res, 200, { ok: true });
      return;
    }
  }

  if (method === "GET" && path === "/api/v1/memberships") {
    const device = requireDevice(deps, req).device;
    const memberships = deps.store.listMemberships(device.id);
    const items = memberships.flatMap((membership) => {
      const peerId =
        membership.host_device_id === device.id
          ? membership.client_device_id
          : membership.host_device_id;
      const peer = deps.store.findDevice(peerId);
      return peer ? [{ membershipId: membership.id, peer: descriptorOf(peer) }] : [];
    });
    sendJson(res, 200, { items });
    return;
  }

  const membershipMatch = /^\/api\/v1\/memberships\/([^/]+)$/.exec(path);
  if (membershipMatch && method === "DELETE") {
    const device = requireDevice(deps, req).device;
    const peerId = decodeURIComponent(membershipMatch[1]!);
    if (!deps.store.hasMembership(device.id, peerId)) {
      throw new ApiError(403, "membership_required", "没有可撤销的授权关系");
    }
    deps.store.revokeMembership(device.id, peerId);
    for (const connection of deps.store.listOpenConnections(device.id)) {
      const involved =
        connection.host_device_id === peerId || connection.client_device_id === peerId;
      if (involved) deps.gateway.closeConnection(connection.id, 4004, "membership revoked");
    }
    sendJson(res, 200, { ok: true });
    return;
  }

  if (method === "POST" && path === "/api/v1/connections") {
    const client = requireDevice(deps, req).device;
    const body = await readJson(req);
    const hostDeviceId = requireString(body, "hostDeviceId");
    const result = deps.gateway.openConnection(client, hostDeviceId);
    sendJson(res, 201, result);
    return;
  }

  const connectionMatch = /^\/api\/v1\/connections\/([^/]+)$/.exec(path);
  if (connectionMatch) {
    const device = requireDevice(deps, req).device;
    const connectionId = decodeURIComponent(connectionMatch[1]!);
    const connection = deps.store.getConnection(connectionId);
    if (!connection) throw new ApiError(404, "connection_not_found", "连接不存在");
    if (connection.host_device_id !== device.id && connection.client_device_id !== device.id) {
      throw new ApiError(403, "forbidden", "无权访问该连接");
    }
    if (method === "GET") {
      sendJson(res, 200, connection);
      return;
    }
    if (method === "DELETE") {
      deps.gateway.closeConnection(connectionId, 4001, "closed by device");
      sendJson(res, 200, { ok: true });
      return;
    }
  }

  throw new ApiError(404, "not_found", "未知路径");
}

function requireBearer(req: IncomingMessage): string {
  const header = req.headers.authorization ?? "";
  const match = /^Bearer\s+(.+)$/i.exec(header);
  if (!match) throw new ApiError(401, "unauthorized", "缺少 Bearer token");
  return match[1]!;
}

function requireWeb(deps: ApiDeps, req: IncomingMessage): { accountId: string; displayName?: string } {
  const found = deps.store.findWebSession(requireBearer(req));
  if (!found) throw new ApiError(401, "session_invalid", "Web 会话无效");
  return { accountId: found.account.id, displayName: found.account.display_name ?? undefined };
}

function requireDevice(deps: ApiDeps, req: IncomingMessage): { device: DeviceRow } {
  const device = deps.store.findDeviceByAccessToken(requireBearer(req));
  if (!device) throw new ApiError(401, "device_unauthorized", "设备凭证无效");
  return { device };
}

function requireAuth(deps: ApiDeps, req: IncomingMessage): Auth {
  const token = requireBearer(req);
  const session = deps.store.findWebSession(token);
  if (session) {
    return {
      kind: "web",
      accountId: session.account.id,
      displayName: session.account.display_name ?? undefined,
    };
  }
  const device = deps.store.findDeviceByAccessToken(token);
  if (!device) throw new ApiError(401, "unauthorized", "凭证无效");
  return { kind: "device", device };
}

function parseAddressList(value: unknown): string[] | undefined {
  if (value === undefined) return undefined;
  if (!Array.isArray(value) || value.length > 8) {
    throw new ApiError(422, "invalid_message", "addresses 必须是至多 8 个地址的数组");
  }
  const result: string[] = [];
  for (const entry of value) {
    if (typeof entry !== "string" || !/^[A-Za-z0-9.:[\]-]+:\d{1,5}$/.test(entry)) {
      throw new ApiError(422, "invalid_message", "addresses 元素格式非法");
    }
    result.push(entry);
  }
  return result;
}

function sendError(res: ServerResponse, error: unknown): void {
  const normalized =
    error instanceof ApiError
      ? error
      : error instanceof GatewayError
        ? new ApiError(error.status, error.code, error.message)
        : error instanceof StoreError
          ? mapStoreError(error)
          : new ApiError(500, "internal", error instanceof Error ? error.message : String(error));
  sendJson(res, normalized.status, {
    error: {
      code: normalized.code,
      message: normalized.message,
      retryable: normalized.status >= 500,
    },
  });
}

function mapStoreError(error: StoreError): ApiError {
  switch (error.code) {
    case "device_conflict":
    case "device_revoked":
      return new ApiError(409, error.code, error.message);
    case "refresh_reused":
      return new ApiError(401, error.code, error.message);
    case "invalid_refresh":
      return new ApiError(401, error.code, error.message);
    case "invalid_github_token":
      return new ApiError(401, error.code, error.message);
    case "github_unreachable":
      return new ApiError(502, error.code, error.message);
    case "pairing_invalid_ticket":
    case "device_not_found":
      return new ApiError(404, error.code, error.message);
    case "pairing_expired":
      return new ApiError(410, error.code, error.message);
    case "pairing_code_invalid":
      return new ApiError(401, error.code, error.message);
    case "pairing_attempts_exhausted":
      return new ApiError(429, error.code, error.message);
    case "pairing_already_used":
    case "pairing_not_claimed":
    case "pairing_not_confirmed":
    case "same_account":
      return new ApiError(409, error.code, error.message);
    case "pairing_role_invalid":
      return new ApiError(400, error.code, error.message);
    case "forbidden":
      return new ApiError(403, error.code, error.message);
    default:
      return new ApiError(400, error.code, error.message);
  }
}
