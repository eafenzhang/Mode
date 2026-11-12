import { randomUUID } from "node:crypto";
import { mkdirSync } from "node:fs";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";

import type { DeviceDescriptor } from "@mode/remote-core";

import { randomToken, sha256Hex } from "./crypto.js";
import { syncMemberships } from "./membershipSync.js";
import { migrateSchema } from "./schema.js";

import {
  ACCESS_TOKEN_TTL_MS,
  REFRESH_TOKEN_TTL_MS,
  WEB_SESSION_TTL_MS,
  StoreError,
  type AccountRow,
  type ConnectionRow,
  type DeviceCredential,
  type DeviceRow,
  type MembershipRow,
} from "./storeTypes.js";

export {
  ACCESS_TOKEN_TTL_MS,
  REFRESH_TOKEN_TTL_MS,
  WEB_SESSION_TTL_MS,
  StoreError,
} from "./storeTypes.js";
export type {
  AccountRow,
  ConnectionRow,
  DeviceCredential,
  DeviceRow,
  MembershipRow,
} from "./storeTypes.js";
export class CloudStore {
  private readonly db: DatabaseSync;
  constructor(dbPath: string) {
    if (dbPath !== ":memory:") mkdirSync(path.dirname(dbPath), { recursive: true });
    this.db = new DatabaseSync(dbPath);
    this.db.exec(
      "PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;",
    );
    migrateSchema(this.db);
  }
  close(): void {
    this.db.close();
  }

  /** 存活探针：只读查询，不暴露任何业务数据。 */
  ping(): boolean {
    try {
      this.db.prepare("SELECT 1").get();
      return true;
    } catch {
      return false;
    }
  }
  createOrFindAccount(provider: string, anchor: string, displayName?: string): AccountRow {
    const existing = this.findAccountByAnchor(provider, anchor);
    if (existing) return existing;
    const now = Date.now();
    const id = randomUUID();
    try {
      this.db
        .prepare(
          "INSERT INTO accounts (id, provider, provider_user_hash, display_name, created_at, status) VALUES (?, ?, ?, ?, ?, 'active')",
        )
        .run(id, provider, anchor, displayName ?? null, now);
    } catch (error) {
      // 并发首次登录可能同时插入；唯一约束失败时复用已建账号。
      const raced = this.findAccountByAnchor(provider, anchor);
      if (raced) return raced;
      throw error;
    }
    return this.findAccountByAnchor(provider, anchor)!;
  }
  findAccountByAnchor(provider: string, anchor: string): AccountRow | null {
    return (
      (this.db
        .prepare("SELECT * FROM accounts WHERE provider = ? AND provider_user_hash = ?")
        .get(provider, anchor) as AccountRow | undefined) ?? null
    );
  }
  createWebSession(accountId: string, label?: string): { token: string; expiresAt: number } {
    const token = randomToken();
    const now = Date.now();
    const expiresAt = now + WEB_SESSION_TTL_MS;
    this.db
      .prepare(
        "INSERT INTO web_sessions (id, account_id, token_hash, client_label, created_at, last_used_at, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
      )
      .run(randomUUID(), accountId, sha256Hex(token), label ?? null, now, now, expiresAt);
    return { token, expiresAt };
  }
  findWebSession(token: string): { account: AccountRow; sessionId: string } | null {
    const hash = sha256Hex(token);
    const row = this.db
      .prepare(
        "SELECT s.id AS session_id, a.* FROM web_sessions s JOIN accounts a ON a.id = s.account_id WHERE s.token_hash = ? AND s.revoked_at IS NULL AND s.expires_at > ?",
      )
      .get(hash, Date.now()) as (AccountRow & { session_id: string }) | undefined;
    if (!row) return null;
    this.db
      .prepare("UPDATE web_sessions SET last_used_at = ? WHERE id = ?")
      .run(Date.now(), row.session_id);
    const account: AccountRow = {
      id: row.id,
      provider: row.provider,
      provider_user_hash: row.provider_user_hash,
      display_name: row.display_name,
      created_at: row.created_at,
      status: row.status,
    };
    return { account, sessionId: row.session_id };
  }
  rotateWebSession(token: string): { token: string; expiresAt: number } | null {
    const found = this.findWebSession(token);
    if (!found) return null;
    this.revokeWebSession(token);
    return this.createWebSession(found.account.id);
  }
  revokeWebSession(token: string): void {
    this.db
      .prepare("UPDATE web_sessions SET revoked_at = ? WHERE token_hash = ? AND revoked_at IS NULL")
      .run(Date.now(), sha256Hex(token));
  }
  upsertDevice(input: {
    accountId: string;
    descriptor: DeviceDescriptor;
    addresses?: string[];
  }): DeviceRow {
    const now = Date.now();
    const descriptor = input.descriptor;
    return this.transaction(() => {
      const existing = this.findDevice(descriptor.deviceId);
      if (existing) {
        if (existing.account_id !== input.accountId) {
          throw new StoreError("device_conflict", "设备已属于其他账号");
        }
        if (existing.role !== descriptor.role || existing.identity_key !== descriptor.identityKey) {
          throw new StoreError("device_conflict", "同一 deviceId 不允许更换角色或身份公钥");
        }
        if (existing.revoked_at !== null) {
          throw new StoreError("device_revoked", "设备已撤销，不能重新注册");
        }
        this.db
          .prepare(
            "UPDATE devices SET name = ?, platform = ?, client_version = ?, host_version = ?, addresses = ?, last_seen_at = ? WHERE id = ?",
          )
          .run(
            descriptor.name,
            descriptor.platform,
            descriptor.clientVersion ?? null,
            descriptor.hostVersion ?? null,
            input.addresses ? JSON.stringify(input.addresses) : existing.addresses,
            now,
            descriptor.deviceId,
          );
      } else {
        this.db
          .prepare(
            "INSERT INTO devices (id, account_id, name, role, platform, identity_key, client_version, host_version, addresses, inbound_only, pairing_restricted, created_at, last_seen_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 0, 0, ?, ?)",
          )
          .run(
            descriptor.deviceId,
            input.accountId,
            descriptor.name,
            descriptor.role,
            descriptor.platform,
            descriptor.identityKey,
            descriptor.clientVersion ?? null,
            descriptor.hostVersion ?? null,
            input.addresses ? JSON.stringify(input.addresses) : null,
            now,
            now,
          );
      }
      syncMemberships(this.db, input.accountId, descriptor.deviceId);
      return this.findDevice(descriptor.deviceId)!;
    });
  }
  findDevice(deviceId: string): DeviceRow | null {
    return (
      (this.db.prepare("SELECT * FROM devices WHERE id = ?").get(deviceId) as
        | DeviceRow
        | undefined) ?? null
    );
  }
  findDeviceByAccessToken(token: string): DeviceRow | null {
    const row = this.db
      .prepare(
        "SELECT d.* FROM device_credentials c JOIN devices d ON d.id = c.device_id WHERE c.access_hash = ? AND c.revoked_at IS NULL AND c.access_expires_at > ? AND d.revoked_at IS NULL",
      )
      .get(sha256Hex(token), Date.now()) as DeviceRow | undefined;
    return row ?? null;
  }
  listPeerDevices(deviceId: string): DeviceRow[] {
    return this.db
      .prepare(
        `SELECT d.* FROM devices d
         JOIN memberships m ON (m.host_device_id = d.id OR m.client_device_id = d.id)
         WHERE (m.host_device_id = ? OR m.client_device_id = ?)
           AND d.id <> ? AND m.revoked_at IS NULL AND d.revoked_at IS NULL
         ORDER BY d.id ASC`,
      )
      .all(deviceId, deviceId, deviceId) as unknown as DeviceRow[];
  }
  hasMembership(deviceA: string, deviceB: string): boolean {
    const row = this.db
      .prepare(
        `SELECT 1 FROM memberships
         WHERE revoked_at IS NULL AND (
           (host_device_id = ? AND client_device_id = ?) OR
           (host_device_id = ? AND client_device_id = ?)
         ) LIMIT 1`,
      )
      .get(deviceA, deviceB, deviceB, deviceA);
    return Boolean(row);
  }

  issueDeviceCredentials(deviceId: string, familyId: string = randomUUID()): DeviceCredential {
    const accessToken = randomToken();
    const refreshToken = randomToken();
    const now = Date.now();
    const accessExpiresAt = now + ACCESS_TOKEN_TTL_MS;
    const refreshExpiresAt = now + REFRESH_TOKEN_TTL_MS;
    this.db
      .prepare(
        `INSERT INTO device_credentials
         (device_id, access_hash, access_expires_at, refresh_hash, refresh_expires_at, family_id, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(device_id) DO UPDATE SET
           access_hash = excluded.access_hash,
           access_expires_at = excluded.access_expires_at,
           refresh_hash = excluded.refresh_hash,
           refresh_expires_at = excluded.refresh_expires_at,
           family_id = excluded.family_id,
           created_at = excluded.created_at,
           rotated_at = NULL,
           revoked_at = NULL`,
      )
      .run(
        deviceId,
        sha256Hex(accessToken),
        accessExpiresAt,
        sha256Hex(refreshToken),
        refreshExpiresAt,
        familyId,
        now,
      );
    return { accessToken, accessExpiresAt, refreshToken, refreshExpiresAt };
  }

  rotateDeviceRefresh(deviceId: string, refreshToken: string): DeviceCredential {
    const hash = sha256Hex(refreshToken);
    const outcome = this.transaction(() => {
      const current = this.db
        .prepare(
          "SELECT * FROM device_credentials WHERE device_id = ? AND refresh_hash = ? AND revoked_at IS NULL AND refresh_expires_at > ?",
        )
        .get(deviceId, hash, Date.now()) as { family_id: string; refresh_hash: string } | undefined;
      if (current) {
        const next = this.issueDeviceCredentials(deviceId, current.family_id);
        this.db
          .prepare(
            "INSERT OR REPLACE INTO device_refresh_history (refresh_hash, device_id, family_id, rotated_at) VALUES (?, ?, ?, ?)",
          )
          .run(current.refresh_hash, deviceId, current.family_id, Date.now());
        return { kind: "rotated" as const, next };
      }
      const reused = this.db
        .prepare(
          "SELECT family_id FROM device_refresh_history WHERE refresh_hash = ? AND device_id = ?",
        )
        .get(hash, deviceId) as { family_id: string } | undefined;
      if (reused) {
        // 重放必须提交撤销，不能被外层事务回滚；事务内只写状态，抛错在事务外。
        this.revokeDeviceRows(deviceId);
        return { kind: "reused" as const };
      }
      return { kind: "invalid" as const };
    });
    if (outcome.kind === "reused") {
      throw new StoreError("refresh_reused", "refresh token 重放，token family 已撤销");
    }
    if (outcome.kind === "invalid") {
      throw new StoreError("invalid_refresh", "refresh token 无效或已过期");
    }
    return outcome.next;
  }

  revokeDevice(deviceId: string): void {
    this.transaction(() => this.revokeDeviceRows(deviceId));
  }

  private revokeDeviceRows(deviceId: string): void {
    const now = Date.now();
    this.db
      .prepare("UPDATE devices SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL")
      .run(now, deviceId);
    this.db.prepare("DELETE FROM device_credentials WHERE device_id = ?").run(deviceId);
    this.db
      .prepare(
        "UPDATE memberships SET revoked_at = ? WHERE (host_device_id = ? OR client_device_id = ?) AND revoked_at IS NULL",
      )
      .run(now, deviceId, deviceId);
    this.db
      .prepare(
        "UPDATE connections SET closed_at = ?, close_code = 4004 WHERE (host_device_id = ? OR client_device_id = ?) AND closed_at IS NULL",
      )
      .run(now, deviceId, deviceId);
  }

  revokeMembership(deviceId: string, peerDeviceId: string): void {
    this.db
      .prepare(
        `UPDATE memberships SET revoked_at = ?
         WHERE revoked_at IS NULL AND (
           (host_device_id = ? AND client_device_id = ?) OR
           (host_device_id = ? AND client_device_id = ?)
         )`,
      )
      .run(Date.now(), deviceId, peerDeviceId, peerDeviceId, deviceId);
  }

  listMemberships(deviceId: string): MembershipRow[] {
    return this.db
      .prepare(
        "SELECT * FROM memberships WHERE (host_device_id = ? OR client_device_id = ?) AND revoked_at IS NULL ORDER BY id ASC",
      )
      .all(deviceId, deviceId) as unknown as MembershipRow[];
  }

  createConnection(clientDeviceId: string, hostDeviceId: string): ConnectionRow {
    const id = randomUUID();
    const now = Date.now();
    this.db
      .prepare(
        "INSERT INTO connections (id, host_device_id, client_device_id, created_at) VALUES (?, ?, ?, ?)",
      )
      .run(id, hostDeviceId, clientDeviceId, now);
    return this.getConnection(id)!;
  }

  getConnection(id: string): ConnectionRow | null {
    return (
      (this.db.prepare("SELECT * FROM connections WHERE id = ?").get(id) as
        | ConnectionRow
        | undefined) ?? null
    );
  }

  closeConnection(id: string, code: number): void {
    this.db
      .prepare(
        "UPDATE connections SET closed_at = ?, close_code = ? WHERE id = ? AND closed_at IS NULL",
      )
      .run(Date.now(), code, id);
  }

  listOpenConnections(deviceId: string): ConnectionRow[] {
    return this.db
      .prepare(
        "SELECT * FROM connections WHERE (host_device_id = ? OR client_device_id = ?) AND closed_at IS NULL",
      )
      .all(deviceId, deviceId) as unknown as ConnectionRow[];
  }

  touchDevice(deviceId: string): void {
    this.db.prepare("UPDATE devices SET last_seen_at = ? WHERE id = ?").run(Date.now(), deviceId);
  }
  private transaction<T>(run: () => T): T {
    this.db.exec("BEGIN IMMEDIATE");
    try {
      const result = run();
      this.db.exec("COMMIT");
      return result;
    } catch (error) {
      this.db.exec("ROLLBACK");
      throw error;
    }
  }
}
