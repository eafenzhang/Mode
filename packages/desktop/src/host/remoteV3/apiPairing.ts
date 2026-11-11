import type {
  RemoteV3PairingStart,
  RemoteV3PairingState,
} from "@mode/shared";

import { readError, readJson, RemoteV3ApiError } from "./api.js";
import type { RemoteV3Tokens } from "./identity.js";

function parsePairingState(body: Record<string, unknown>): RemoteV3PairingState {
  const status = body.status;
  if (
    status !== "pending" &&
    status !== "claimed" &&
    status !== "confirmed" &&
    status !== "canceled" &&
    status !== "expired"
  ) {
    throw new Error("配对状态响应非法");
  }
  return {
    status,
    expiresAt: typeof body.expiresAt === "number" ? body.expiresAt : 0,
    hostName: typeof body.hostName === "string" ? body.hostName : null,
    clientName: typeof body.clientName === "string" ? body.clientName : null,
  };
}

/** Host 角色创建配对会话（docs/specs/remote-control-v3-pairing.md）。 */
export async function beginRemoteV3Pairing(input: {
  serverUrl: string;
  accessToken: string;
  fetchImpl?: typeof fetch;
}): Promise<RemoteV3PairingStart> {
  const fetchImpl = input.fetchImpl ?? fetch;
  const response = await fetchImpl(`${input.serverUrl}/api/v1/pairing/sessions`, {
    method: "POST",
    headers: { authorization: `Bearer ${input.accessToken}` },
  });
  const body = await readJson(response);
  if (!response.ok) throw new RemoteV3ApiError(response.status, readError(body, "创建配对失败"));
  if (
    typeof body.pairingId !== "string" ||
    typeof body.ticket !== "string" ||
    typeof body.expiresAt !== "number"
  ) {
    throw new Error("配对创建响应缺少字段");
  }
  return { pairingId: body.pairingId, ticket: body.ticket, expiresAt: body.expiresAt };
}

/** Host 角色本机确认；成功返回轮换后的 Host 凭证。 */
export async function confirmRemoteV3Pairing(input: {
  serverUrl: string;
  accessToken: string;
  pairingId: string;
  code: string;
  fetchImpl?: typeof fetch;
}): Promise<RemoteV3Tokens> {
  const fetchImpl = input.fetchImpl ?? fetch;
  const response = await fetchImpl(
    `${input.serverUrl}/api/v1/pairing/sessions/${encodeURIComponent(input.pairingId)}/confirm`,
    {
      method: "POST",
      headers: {
        "content-type": "application/json",
        authorization: `Bearer ${input.accessToken}`,
      },
      body: JSON.stringify({ code: input.code }),
    },
  );
  const body = await readJson(response);
  if (!response.ok) throw new RemoteV3ApiError(response.status, readError(body, "确认配对失败"));
  if (typeof body.accessToken !== "string" || typeof body.refreshToken !== "string") {
    throw new Error("配对确认响应缺少 token");
  }
  return {
    accessToken: body.accessToken,
    accessExpiresAt: typeof body.accessExpiresAt === "number" ? body.accessExpiresAt : 0,
    refreshToken: body.refreshToken,
    refreshExpiresAt: typeof body.refreshExpiresAt === "number" ? body.refreshExpiresAt : 0,
  };
}

export async function getRemoteV3Pairing(input: {
  serverUrl: string;
  accessToken: string;
  pairingId: string;
  fetchImpl?: typeof fetch;
}): Promise<RemoteV3PairingState> {
  const fetchImpl = input.fetchImpl ?? fetch;
  const response = await fetchImpl(
    `${input.serverUrl}/api/v1/pairing/sessions/${encodeURIComponent(input.pairingId)}`,
    { headers: { authorization: `Bearer ${input.accessToken}` } },
  );
  const body = await readJson(response);
  if (!response.ok) throw new RemoteV3ApiError(response.status, readError(body, "读取配对状态失败"));
  return parsePairingState(body);
}

export async function cancelRemoteV3Pairing(input: {
  serverUrl: string;
  accessToken: string;
  pairingId: string;
  fetchImpl?: typeof fetch;
}): Promise<RemoteV3PairingState> {
  const fetchImpl = input.fetchImpl ?? fetch;
  const response = await fetchImpl(
    `${input.serverUrl}/api/v1/pairing/sessions/${encodeURIComponent(input.pairingId)}/cancel`,
    {
      method: "POST",
      headers: { authorization: `Bearer ${input.accessToken}` },
    },
  );
  const body = await readJson(response);
  if (!response.ok) throw new RemoteV3ApiError(response.status, readError(body, "取消配对失败"));
  return parsePairingState(body);
}
