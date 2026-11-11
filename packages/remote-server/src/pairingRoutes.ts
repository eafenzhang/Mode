import type { IncomingMessage, ServerResponse } from "node:http";

import { ApiError, readJson, requireString, sendJson, type ApiDeps } from "./apiSupport.js";
import type { DeviceRow } from "./store.js";

/**
 * 跨账号配对路由（docs/specs/remote-control-v3-pairing.md）。
 *
 * 从 `api.ts` 拆出只为控制单文件体量；鉴权函数由调用方注入，保持与其它 device
 * 路由同一份 token 校验实现。返回 true 表示路径已被本模块处理。
 */
export async function handlePairingRoute(
  deps: ApiDeps,
  req: IncomingMessage,
  res: ServerResponse,
  method: string,
  path: string,
  requireDevice: (req: IncomingMessage) => { device: DeviceRow },
): Promise<boolean> {
  if (method === "POST" && path === "/api/v1/pairing/sessions") {
    const device = requireDevice(req).device;
    const { pairing, ticket } = deps.store.pairing.createPairingSession(
      device.id,
      deps.pairingTtlMs ?? 180_000,
    );
    sendJson(res, 201, { pairingId: pairing.id, ticket, expiresAt: pairing.expires_at });
    return true;
  }

  if (method === "POST" && path === "/api/v1/pairing/sessions/claim") {
    const device = requireDevice(req).device;
    const body = await readJson(req);
    const ticket = requireString(body, "ticket");
    const { pairing, code } = deps.store.pairing.claimPairingSession({
      ticket,
      clientDeviceId: device.id,
    });
    const host = deps.store.findDevice(pairing.host_device_id);
    sendJson(res, 200, {
      pairingId: pairing.id,
      verificationCode: code,
      hostName: host?.name ?? "",
      expiresAt: pairing.expires_at,
    });
    return true;
  }

  const match = /^\/api\/v1\/pairing\/sessions\/([^/]+)(?:\/(confirm|redeem|cancel))?$/.exec(path);
  if (!match) return false;

  const pairingId = decodeURIComponent(match[1]!);
  const action = match[2];
  const device = requireDevice(req).device;

  if (!action && method === "GET") {
    const pairing = deps.store.pairing.findPairingSession(pairingId);
    if (!pairing) throw new ApiError(404, "pairing_invalid_ticket", "配对会话不存在");
    if (pairing.host_device_id !== device.id && pairing.client_device_id !== device.id) {
      throw new ApiError(403, "forbidden", "无权查看该配对会话");
    }
    const expired = pairing.status !== "confirmed" && Date.now() > pairing.expires_at;
    sendJson(res, 200, {
      status: expired ? "expired" : pairing.status,
      expiresAt: pairing.expires_at,
      hostName: deps.store.findDevice(pairing.host_device_id)?.name ?? null,
      clientName: pairing.client_device_id
        ? (deps.store.findDevice(pairing.client_device_id)?.name ?? null)
        : null,
    });
    return true;
  }

  if (method === "POST" && action === "confirm") {
    const body = await readJson(req);
    const code = requireString(body, "code");
    const { credentials } = deps.store.pairing.confirmPairingSession({
      id: pairingId,
      hostDeviceId: device.id,
      code,
    });
    sendJson(res, 200, { status: "confirmed", ...credentials });
    return true;
  }

  if (method === "POST" && action === "redeem") {
    const credentials = deps.store.pairing.redeemPairingSession({
      id: pairingId,
      clientDeviceId: device.id,
    });
    sendJson(res, 200, credentials);
    return true;
  }

  if (method === "POST" && action === "cancel") {
    const pairing = deps.store.pairing.cancelPairingSession({
      id: pairingId,
      deviceId: device.id,
    });
    sendJson(res, 200, { status: pairing.status });
    return true;
  }

  return false;
}
