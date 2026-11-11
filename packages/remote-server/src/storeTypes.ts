export const ACCESS_TOKEN_TTL_MS = 60 * 60 * 1000;
export const REFRESH_TOKEN_TTL_MS = 30 * 24 * 60 * 60 * 1000;
export const WEB_SESSION_TTL_MS = 30 * 24 * 60 * 60 * 1000;

export class StoreError extends Error {
  readonly code: string;
  constructor(code: string, message: string) {
    super(message);
    this.name = "StoreError";
    this.code = code;
  }
}

export interface AccountRow {
  id: string;
  provider: string;
  provider_user_hash: string;
  display_name: string | null;
  created_at: number;
  status: string;
}

export interface DeviceRow {
  id: string;
  account_id: string;
  name: string;
  role: "host" | "client";
  platform: string;
  identity_key: string;
  client_version: string | null;
  host_version: string | null;
  addresses: string | null;
  inbound_only: number;
  pairing_restricted: number;
  created_at: number;
  last_seen_at: number | null;
  revoked_at: number | null;
}

export interface MembershipRow {
  id: string;
  host_device_id: string;
  client_device_id: string;
  created_at: number;
  revoked_at: number | null;
}

export interface ConnectionRow {
  id: string;
  host_device_id: string;
  client_device_id: string;
  created_at: number;
  closed_at: number | null;
  close_code: number | null;
  transport: string | null;
}

export interface DeviceCredential {
  accessToken: string;
  accessExpiresAt: number;
  refreshToken: string;
  refreshExpiresAt: number;
}
