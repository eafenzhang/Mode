/** 跨账号配对（docs/specs/remote-control-v3-pairing.md）浏览器 Client 侧响应解析。 */

export interface WebPairingClaim {
  pairingId: string;
  verificationCode: string;
  hostName: string;
  expiresAt: number;
}

export type WebPairingStatus = "pending" | "claimed" | "confirmed" | "canceled" | "expired";

export interface WebPairingState {
  status: WebPairingStatus;
  expiresAt: number;
  hostName: string | null;
  clientName: string | null;
}

export interface WebDeviceCredentials {
  accessToken: string;
  accessExpiresAt: number;
  refreshToken: string;
  refreshExpiresAt: number;
}

/** device token 请求（401 刷新重试由调用方负责）。 */
export type PairingRequest = (
  path: string,
  init?: { method?: string; body?: unknown },
) => Promise<Record<string, unknown>>;

export function parseDeviceCredentials(body: Record<string, unknown>): WebDeviceCredentials {
  if (
    typeof body.accessToken !== "string" ||
    typeof body.refreshToken !== "string" ||
    typeof body.accessExpiresAt !== "number" ||
    typeof body.refreshExpiresAt !== "number"
  ) {
    throw new Error("配对凭证响应缺少字段");
  }
  return {
    accessToken: body.accessToken,
    refreshToken: body.refreshToken,
    accessExpiresAt: body.accessExpiresAt,
    refreshExpiresAt: body.refreshExpiresAt,
  };
}

export async function claimPairing(
  request: PairingRequest,
  ticket: string,
): Promise<WebPairingClaim> {
  const body = await request("/api/v1/pairing/sessions/claim", {
    method: "POST",
    body: { ticket },
  });
  if (
    typeof body.pairingId !== "string" ||
    typeof body.verificationCode !== "string" ||
    typeof body.expiresAt !== "number"
  ) {
    throw new Error("配对 claim 响应缺少字段");
  }
  return {
    pairingId: body.pairingId,
    verificationCode: body.verificationCode,
    hostName: typeof body.hostName === "string" ? body.hostName : "",
    expiresAt: body.expiresAt,
  };
}

export async function getPairing(
  request: PairingRequest,
  pairingId: string,
): Promise<WebPairingState> {
  const body = await request(`/api/v1/pairing/sessions/${encodeURIComponent(pairingId)}`);
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

export async function redeemPairing(
  request: PairingRequest,
  pairingId: string,
): Promise<WebDeviceCredentials> {
  return parseDeviceCredentials(
    await request(`/api/v1/pairing/sessions/${encodeURIComponent(pairingId)}/redeem`, {
      method: "POST",
    }),
  );
}

export async function cancelPairing(
  request: PairingRequest,
  pairingId: string,
): Promise<void> {
  await request(`/api/v1/pairing/sessions/${encodeURIComponent(pairingId)}/cancel`, {
    method: "POST",
  });
}
