import type { DatabaseSync } from "node:sqlite";

export const SCHEMA_VERSION = 2;

/** 远程控制 v3 的账号/设备/membership/连接 schema；只增不改。 */
export function migrateSchema(db: DatabaseSync): void {
  const row = db.prepare("PRAGMA user_version").get() as { user_version?: number } | undefined;
  const current = row?.user_version ?? 0;
  if (current > SCHEMA_VERSION) {
    throw new Error(`数据库 schema 版本 ${current} 高于当前支持的 ${SCHEMA_VERSION}`);
  }
  if (current === SCHEMA_VERSION) return;
  // v1 的建表语句全为 IF NOT EXISTS，可安全接管未标记版本的旧库。
  db.exec(`
      CREATE TABLE IF NOT EXISTS accounts (
        id TEXT PRIMARY KEY,
        provider TEXT NOT NULL,
        provider_user_hash TEXT NOT NULL,
        display_name TEXT,
        created_at INTEGER NOT NULL,
        status TEXT NOT NULL,
        UNIQUE(provider, provider_user_hash)
      );
      CREATE TABLE IF NOT EXISTS web_sessions (
        id TEXT PRIMARY KEY,
        account_id TEXT NOT NULL,
        token_hash TEXT NOT NULL UNIQUE,
        client_label TEXT,
        created_at INTEGER NOT NULL,
        last_used_at INTEGER,
        expires_at INTEGER NOT NULL,
        revoked_at INTEGER,
        FOREIGN KEY(account_id) REFERENCES accounts(id)
      );
      CREATE TABLE IF NOT EXISTS devices (
        id TEXT PRIMARY KEY,
        account_id TEXT NOT NULL,
        name TEXT NOT NULL,
        role TEXT NOT NULL CHECK(role IN ('host','client')),
        platform TEXT NOT NULL,
        identity_key TEXT NOT NULL,
        client_version TEXT,
        host_version TEXT,
        addresses TEXT,
        inbound_only INTEGER NOT NULL DEFAULT 0,
        pairing_restricted INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL,
        last_seen_at INTEGER,
        revoked_at INTEGER,
        FOREIGN KEY(account_id) REFERENCES accounts(id)
      );
      CREATE INDEX IF NOT EXISTS idx_devices_account ON devices(account_id);
      CREATE TABLE IF NOT EXISTS device_credentials (
        device_id TEXT PRIMARY KEY,
        access_hash TEXT NOT NULL UNIQUE,
        access_expires_at INTEGER NOT NULL,
        refresh_hash TEXT NOT NULL UNIQUE,
        refresh_expires_at INTEGER NOT NULL,
        family_id TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        rotated_at INTEGER,
        revoked_at INTEGER,
        FOREIGN KEY(device_id) REFERENCES devices(id)
      );
      CREATE TABLE IF NOT EXISTS device_refresh_history (
        refresh_hash TEXT PRIMARY KEY,
        device_id TEXT NOT NULL,
        family_id TEXT NOT NULL,
        rotated_at INTEGER NOT NULL
      );
      CREATE TABLE IF NOT EXISTS memberships (
        id TEXT PRIMARY KEY,
        host_device_id TEXT NOT NULL,
        client_device_id TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        revoked_at INTEGER,
        UNIQUE(host_device_id, client_device_id)
      );
      CREATE INDEX IF NOT EXISTS idx_memberships_host ON memberships(host_device_id);
      CREATE INDEX IF NOT EXISTS idx_memberships_client ON memberships(client_device_id);
      CREATE TABLE IF NOT EXISTS connections (
        id TEXT PRIMARY KEY,
        host_device_id TEXT NOT NULL,
        client_device_id TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        closed_at INTEGER,
        close_code INTEGER,
        transport TEXT
      );
      CREATE INDEX IF NOT EXISTS idx_connections_host ON connections(host_device_id);
      CREATE INDEX IF NOT EXISTS idx_connections_client ON connections(client_device_id);
    `);
  db.exec(`PRAGMA user_version = ${SCHEMA_VERSION}`);
}
