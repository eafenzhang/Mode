/**
 * Mode Cloud 控制面持久化（docs/specs/cloud-personal-accounts.md，v2）。
 *
 * 只保存账号锚点哈希、设备元数据、设备公钥和令牌（设备令牌/中继令牌只存哈希，
 * 设备访问令牌只存 AES-256-GCM 密文）；不保存配对码、远控票据或任何业务内容。
 */
import { randomUUID } from "node:crypto";
import { DatabaseSync } from "node:sqlite";

import { CloudAuthStore } from "./cloudAuthStore.js";
import { migrateCloudDatabase } from "./cloudSchema.js";

export interface CloudAccountRow {
  id: string;
  provider: string;
  providerUserHash: string;
  displayName: string | null;
  status: string;
}

export interface CloudDeviceRow {
  id: string;
  accountId: string;
  name: string;
  platform: string;
  version: string | null;
  publicKey: string;
  relayTunnelId: string;
  /** 设备自报可直连地址（host:port）。 */
  addresses: string[];
  /** 设备访问令牌密文（AES-256-GCM）；解密只在目录序列化时进行。 */
  accessTokenCipher: string | null;
  createdAt: number;
  lastSeenAt: number | null;
  revokedAt: number | null;
}

export interface CloudDeviceWithAccount {
  device: CloudDeviceRow;
  account: CloudAccountRow;
}

export type CloudEnrollResult =
  | { ok: true; accountId: string; deviceId: string; tookOver: boolean }
  | { ok: false; error: "account_disabled" | "device_conflict" | "tunnel_conflict" };

export interface CloudEnrollInput {
  provider: string;
  providerUserHash: string;
  displayName?: string;
  device: {
    id: string;
    name: string;
    platform: string;
    version?: string;
    publicKey: string;
    relayTunnelId: string;
    /** 本机 API 令牌密文（调用方先用 secretBox 加密）。 */
    accessTokenCipher: string;
    addresses?: string[];
  };
  tokenHash: string;
  /** 每设备中继令牌哈希（docs/specs/cloud-personal-accounts.md）； enroll 时轮换。 */
  relayTokenHash: string;
  now: number;
}

function columnText(value: unknown): string {
  return typeof value === "string" ? value : String(value ?? "");
}

function columnNumber(value: unknown): number | null {
  return typeof value === "number" ? value : null;
}

function parseAddresses(value: unknown): string[] {
  if (typeof value !== "string" || !value) {
    return [];
  }
  try {
    const parsed: unknown = JSON.parse(value);
    if (!Array.isArray(parsed)) {
      return [];
    }
    return parsed.filter((item): item is string => typeof item === "string" && item.length > 0);
  } catch {
    return [];
  }
}

function mapDevice(row: Record<string, unknown>): CloudDeviceRow {
  return {
    id: columnText(row.id),
    accountId: columnText(row.account_id),
    name: columnText(row.name),
    platform: columnText(row.platform),
    version: typeof row.version === "string" ? row.version : null,
    publicKey: columnText(row.public_key),
    relayTunnelId: columnText(row.relay_tunnel_id),
    addresses: parseAddresses(row.addresses),
    accessTokenCipher:
      typeof row.access_token_cipher === "string" && row.access_token_cipher
        ? row.access_token_cipher
        : null,
    createdAt: columnNumber(row.created_at) ?? 0,
    lastSeenAt: columnNumber(row.last_seen_at),
    revokedAt: columnNumber(row.revoked_at),
  };
}

function mapAccount(row: Record<string, unknown>): CloudAccountRow {
  return {
    id: columnText(row.id),
    provider: columnText(row.provider),
    providerUserHash: columnText(row.provider_user_hash),
    displayName: typeof row.display_name === "string" ? row.display_name : null,
    status: columnText(row.status),
  };
}

export class CloudStore {
  private readonly db: DatabaseSync;
  /** 个人账号凭据（GitHub 锚点 + 人类会话）；与设备目录同一个 SQLite，唯一真相不变。 */
  readonly auth: CloudAuthStore;

  constructor(path: string) {
    this.db = new DatabaseSync(path);
    this.db.exec("PRAGMA journal_mode=WAL;");
    this.db.exec("PRAGMA foreign_keys=ON;");
    migrateCloudDatabase(this.db);
    this.auth = new CloudAuthStore(this.db);
  }

  close(): void {
    this.db.close();
  }

  private transaction<T>(operation: () => T): T {
    this.db.exec("BEGIN IMMEDIATE");
    try {
      const result = operation();
      this.db.exec("COMMIT");
      return result;
    } catch (error) {
      this.db.exec("ROLLBACK");
      throw error;
    }
  }

  enroll(input: CloudEnrollInput): CloudEnrollResult {
    return this.transaction(() => {
      let account = this.db
        .prepare("SELECT * FROM cloud_accounts WHERE provider = ? AND provider_user_hash = ?")
        .get(input.provider, input.providerUserHash) as Record<string, unknown> | undefined;
      if (!account) {
        this.db
          .prepare(
            "INSERT INTO cloud_accounts (id, provider, provider_user_hash, display_name, created_at, status) VALUES (?, ?, ?, ?, ?, 'active')",
          )
          .run(
            randomUUID(),
            input.provider,
            input.providerUserHash,
            input.displayName ?? null,
            input.now,
          );
        account = this.db
          .prepare("SELECT * FROM cloud_accounts WHERE provider = ? AND provider_user_hash = ?")
          .get(input.provider, input.providerUserHash) as Record<string, unknown>;
      }
      const accountRow = mapAccount(account);
      if (accountRow.status !== "active") {
        return { ok: false, error: "account_disabled" } as const;
      }

      const existingDevice = this.db
        .prepare("SELECT * FROM cloud_devices WHERE id = ?")
        .get(input.device.id) as Record<string, unknown> | undefined;
      let tookOver = false;
      if (existingDevice && columnText(existingDevice.account_id) !== accountRow.id) {
        // 设备属于其他账号时默认拒绝（账号即信任边界）。唯一例外：原属账号是
        // 旧身份（非 github——密码/微信登录已删除，该账号已无法登录）的遗留设备，
        // 允许首个 GitHub 账号认领，否则升级部署后存量设备会被永久锁死。
        const owner = this.db
          .prepare("SELECT provider FROM cloud_accounts WHERE id = ?")
          .get(columnText(existingDevice.account_id)) as Record<string, unknown> | undefined;
        if (columnText(owner?.provider) === "github") {
          return { ok: false, error: "device_conflict" } as const;
        }
        tookOver = true;
      }
      const tunnelOwner = this.db
        .prepare("SELECT id FROM cloud_devices WHERE relay_tunnel_id = ? AND id <> ?")
        .get(input.device.relayTunnelId, input.device.id) as Record<string, unknown> | undefined;
      if (tunnelOwner) {
        return { ok: false, error: "tunnel_conflict" } as const;
      }

      const addressesJson = JSON.stringify(input.device.addresses ?? []);
      if (existingDevice) {
        this.db
          .prepare(
            "UPDATE cloud_devices SET account_id = ?, name = ?, platform = ?, version = ?, public_key = ?, relay_tunnel_id = ?, relay_token_hash = ?, access_token_cipher = ?, addresses = ?, last_seen_at = ?, revoked_at = NULL WHERE id = ?",
          )
          .run(
            accountRow.id,
            input.device.name,
            input.device.platform,
            input.device.version ?? null,
            input.device.publicKey,
            input.device.relayTunnelId,
            input.relayTokenHash,
            input.device.accessTokenCipher,
            addressesJson,
            input.now,
            input.device.id,
          );
      } else {
        this.db
          .prepare(
            "INSERT INTO cloud_devices (id, account_id, name, platform, version, public_key, relay_tunnel_id, relay_token_hash, access_token_cipher, addresses, created_at, last_seen_at, revoked_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)",
          )
          .run(
            input.device.id,
            accountRow.id,
            input.device.name,
            input.device.platform,
            input.device.version ?? null,
            input.device.publicKey,
            input.device.relayTunnelId,
            input.relayTokenHash,
            input.device.accessTokenCipher,
            addressesJson,
            input.now,
            input.now,
          );
      }
      this.db
        .prepare(
          "INSERT INTO cloud_device_tokens (device_id, token_hash, created_at, last_used_at) VALUES (?, ?, ?, ?) ON CONFLICT(device_id) DO UPDATE SET token_hash = excluded.token_hash, created_at = excluded.created_at, last_used_at = excluded.last_used_at",
        )
        .run(input.device.id, input.tokenHash, input.now, input.now);

      return { ok: true, accountId: accountRow.id, deviceId: input.device.id, tookOver } as const;
    });
  }

  findDeviceByTokenHash(tokenHash: string): CloudDeviceWithAccount | null {
    const row = this.db
      .prepare(
        `SELECT d.*, a.id AS account_id_join, a.provider AS account_provider,
                a.provider_user_hash AS account_provider_user_hash,
                a.display_name AS account_display_name, a.status AS account_status
           FROM cloud_device_tokens t
           JOIN cloud_devices d ON d.id = t.device_id
           JOIN cloud_accounts a ON a.id = d.account_id
          WHERE t.token_hash = ? AND d.revoked_at IS NULL`,
      )
      .get(tokenHash) as Record<string, unknown> | undefined;
    if (!row) {
      return null;
    }
    this.db
      .prepare("UPDATE cloud_device_tokens SET last_used_at = ? WHERE device_id = ?")
      .run(Date.now(), columnText(row.id));
    return {
      device: mapDevice(row),
      account: {
        id: columnText(row.account_id),
        provider: columnText(row.account_provider),
        providerUserHash: columnText(row.account_provider_user_hash),
        displayName:
          typeof row.account_display_name === "string" ? row.account_display_name : null,
        status: columnText(row.account_status),
      },
    };
  }

  /** 隧道鉴权（docs/specs/cloud-personal-accounts.md）：按 tunnelId 取设备的中继令牌哈希。 */
  findDeviceByTunnelId(tunnelId: string): {
    id: string;
    relayTokenHash: string | null;
    revoked: boolean;
  } | null {
    const row = this.db
      .prepare("SELECT id, relay_token_hash, revoked_at FROM cloud_devices WHERE relay_tunnel_id = ?")
      .get(tunnelId) as Record<string, unknown> | undefined;
    if (!row) {
      return null;
    }
    return {
      id: columnText(row.id),
      relayTokenHash:
        typeof row.relay_token_hash === "string" && row.relay_token_hash
          ? row.relay_token_hash
          : null,
      revoked: row.revoked_at !== null && row.revoked_at !== undefined,
    };
  }

  listDevices(accountId: string): CloudDeviceRow[] {
    const rows = this.db
      .prepare(
        "SELECT * FROM cloud_devices WHERE account_id = ? AND revoked_at IS NULL ORDER BY created_at ASC",
      )
      .all(accountId) as Record<string, unknown>[];
    return rows.map(mapDevice);
  }

  revokeDevice(accountId: string, deviceId: string): boolean {
    return this.transaction(() => {
      const result = this.db
        .prepare(
          "UPDATE cloud_devices SET revoked_at = ?, relay_token_hash = NULL, access_token_cipher = NULL WHERE id = ? AND account_id = ? AND revoked_at IS NULL",
        )
        .run(Date.now(), deviceId, accountId);
      if (Number(result.changes ?? 0) === 0) {
        return false;
      }
      this.db.prepare("DELETE FROM cloud_device_tokens WHERE device_id = ?").run(deviceId);
      return true;
    });
  }

  stats(): { accounts: number; devices: number } {
    const accounts = this.db.prepare("SELECT COUNT(*) AS count FROM cloud_accounts").get() as
      | Record<string, unknown>
      | undefined;
    const devices = this.db
      .prepare("SELECT COUNT(*) AS count FROM cloud_devices WHERE revoked_at IS NULL")
      .get() as Record<string, unknown> | undefined;
    return {
      accounts: Number(accounts?.count ?? 0),
      devices: Number(devices?.count ?? 0),
    };
  }
}
