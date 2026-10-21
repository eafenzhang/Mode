/**
 * Mode Cloud 控制面持久化（docs/specs/mode-cloud-account-directory.md）。
 *
 * 只保存账号锚点哈希、设备元数据、设备公钥和设备令牌哈希；
 * 不保存 LAN 令牌、配对码、远控票据或任何业务内容。
 */
import { randomUUID } from "node:crypto";
import { DatabaseSync } from "node:sqlite";

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
  createdAt: number;
  lastSeenAt: number | null;
  revokedAt: number | null;
}

export interface CloudDeviceWithAccount {
  device: CloudDeviceRow;
  account: CloudAccountRow;
}

export interface CloudLoginCodeRow {
  codeHash: string;
  createdAt: number;
  expiresAt: number;
  status: string;
  providerUserHash: string | null;
  approvedAt: number | null;
}

export type CloudEnrollResult =
  | { ok: true; accountId: string; deviceId: string }
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
  };
  tokenHash: string;
  now: number;
}

function columnText(value: unknown): string {
  return typeof value === "string" ? value : String(value ?? "");
}

function columnNumber(value: unknown): number | null {
  return typeof value === "number" ? value : null;
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

  constructor(path: string) {
    this.db = new DatabaseSync(path);
    this.db.exec("PRAGMA journal_mode=WAL;");
    this.db.exec("PRAGMA foreign_keys=ON;");
    migrateCloudDatabase(this.db);
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
      if (existingDevice && columnText(existingDevice.account_id) !== accountRow.id) {
        return { ok: false, error: "device_conflict" } as const;
      }
      const tunnelOwner = this.db
        .prepare("SELECT id FROM cloud_devices WHERE relay_tunnel_id = ? AND id <> ?")
        .get(input.device.relayTunnelId, input.device.id) as Record<string, unknown> | undefined;
      if (tunnelOwner) {
        return { ok: false, error: "tunnel_conflict" } as const;
      }

      if (existingDevice) {
        this.db
          .prepare(
            "UPDATE cloud_devices SET account_id = ?, name = ?, platform = ?, version = ?, public_key = ?, relay_tunnel_id = ?, last_seen_at = ?, revoked_at = NULL WHERE id = ?",
          )
          .run(
            accountRow.id,
            input.device.name,
            input.device.platform,
            input.device.version ?? null,
            input.device.publicKey,
            input.device.relayTunnelId,
            input.now,
            input.device.id,
          );
      } else {
        this.db
          .prepare(
            "INSERT INTO cloud_devices (id, account_id, name, platform, version, public_key, relay_tunnel_id, created_at, last_seen_at, revoked_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)",
          )
          .run(
            input.device.id,
            accountRow.id,
            input.device.name,
            input.device.platform,
            input.device.version ?? null,
            input.device.publicKey,
            input.device.relayTunnelId,
            input.now,
            input.now,
          );
      }
      this.db
        .prepare(
          "INSERT INTO cloud_device_tokens (device_id, token_hash, created_at, last_used_at) VALUES (?, ?, ?, ?) ON CONFLICT(device_id) DO UPDATE SET token_hash = excluded.token_hash, created_at = excluded.created_at, last_used_at = excluded.last_used_at",
        )
        .run(input.device.id, input.tokenHash, input.now, input.now);

      return { ok: true, accountId: accountRow.id, deviceId: input.device.id } as const;
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
          "UPDATE cloud_devices SET revoked_at = ? WHERE id = ? AND account_id = ? AND revoked_at IS NULL",
        )
        .run(Date.now(), deviceId, accountId);
      if (Number(result.changes ?? 0) === 0) {
        return false;
      }
      this.db.prepare("DELETE FROM cloud_device_tokens WHERE device_id = ?").run(deviceId);
      return true;
    });
  }

  createLoginCode(input: {
    codeHash: string;
    createdAt: number;
    expiresAt: number;
  }): void {
    this.db
      .prepare(
        "INSERT OR REPLACE INTO cloud_login_codes (code_hash, created_at, expires_at, status, provider_user_hash, approved_at) VALUES (?, ?, ?, 'pending', NULL, NULL)",
      )
      .run(input.codeHash, input.createdAt, input.expiresAt);
    // 只清理一小时前的过期码，避免频繁写入；登录码本身 5 分钟有效。
    this.db
      .prepare("DELETE FROM cloud_login_codes WHERE expires_at < ?")
      .run(input.createdAt - 3_600_000);
  }

  getLoginCode(codeHash: string): CloudLoginCodeRow | null {
    const row = this.db
      .prepare("SELECT * FROM cloud_login_codes WHERE code_hash = ?")
      .get(codeHash) as Record<string, unknown> | undefined;
    if (!row) {
      return null;
    }
    return {
      codeHash: columnText(row.code_hash),
      createdAt: columnNumber(row.created_at) ?? 0,
      expiresAt: columnNumber(row.expires_at) ?? 0,
      status: columnText(row.status),
      providerUserHash:
        typeof row.provider_user_hash === "string" ? row.provider_user_hash : null,
      approvedAt: columnNumber(row.approved_at),
    };
  }

  approveLoginCode(input: {
    codeHash: string;
    providerUserHash: string;
    now: number;
  }): boolean {
    const result = this.db
      .prepare(
        "UPDATE cloud_login_codes SET status = 'approved', provider_user_hash = ?, approved_at = ? WHERE code_hash = ? AND status = 'pending' AND expires_at > ?",
      )
      .run(input.providerUserHash, input.now, input.codeHash, input.now);
    return Number(result.changes ?? 0) > 0;
  }

  /** 已批准且未过期的码只能消费一次；返回账号锚点哈希。 */
  consumeLoginCode(codeHash: string, now: number): string | null {
    return this.transaction(() => {
      const row = this.getLoginCode(codeHash);
      if (!row || row.status !== "approved" || row.expiresAt <= now || !row.providerUserHash) {
        return null;
      }
      this.db
        .prepare("UPDATE cloud_login_codes SET status = 'consumed' WHERE code_hash = ?")
        .run(codeHash);
      return row.providerUserHash;
    });
  }

  getBotBinding(): string | null {
    const row = this.db
      .prepare("SELECT provider_user_hash FROM cloud_bot_binding WHERE id = 1")
      .get() as Record<string, unknown> | undefined;
    return typeof row?.provider_user_hash === "string" ? row.provider_user_hash : null;
  }

  bindBotUser(providerUserHash: string, now: number): void {
    this.db
      .prepare(
        "INSERT INTO cloud_bot_binding (id, provider, provider_user_hash, bound_at) VALUES (1, 'weixin', ?, ?) ON CONFLICT(id) DO UPDATE SET provider_user_hash = excluded.provider_user_hash, bound_at = excluded.bound_at",
      )
      .run(providerUserHash, now);
  }

  saveBotConfig(input: {
    tokenCiphertext: string;
    tokenIv: string;
    tokenTag: string;
    allowedUserHash: string | null;
    now: number;
  }): void {
    this.db
      .prepare(
        "INSERT INTO cloud_bot_config (id, token_ciphertext, token_iv, token_tag, allowed_user_hash, updated_at) VALUES (1, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET token_ciphertext = excluded.token_ciphertext, token_iv = excluded.token_iv, token_tag = excluded.token_tag, allowed_user_hash = excluded.allowed_user_hash, updated_at = excluded.updated_at",
      )
      .run(
        input.tokenCiphertext,
        input.tokenIv,
        input.tokenTag,
        input.allowedUserHash,
        input.now,
      );
  }

  loadBotConfig(): {
    tokenCiphertext: string;
    tokenIv: string;
    tokenTag: string;
    allowedUserHash: string | null;
  } | null {
    const row = this.db
      .prepare("SELECT * FROM cloud_bot_config WHERE id = 1")
      .get() as Record<string, unknown> | undefined;
    if (!row) {
      return null;
    }
    return {
      tokenCiphertext: columnText(row.token_ciphertext),
      tokenIv: columnText(row.token_iv),
      tokenTag: columnText(row.token_tag),
      allowedUserHash:
        typeof row.allowed_user_hash === "string" ? row.allowed_user_hash : null,
    };
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
