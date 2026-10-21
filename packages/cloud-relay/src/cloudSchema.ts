/**
 * Mode Cloud SQLite schema。从 cloudStore 抽出以保持单文件行数限制。
 */
import type { DatabaseSync } from "node:sqlite";

export function migrateCloudDatabase(db: DatabaseSync): void {
  db.exec(`
    CREATE TABLE IF NOT EXISTS cloud_schema_migration (
      version INTEGER PRIMARY KEY,
      applied_at INTEGER NOT NULL,
      checksum TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS cloud_accounts (
      id TEXT PRIMARY KEY,
      provider TEXT NOT NULL,
      provider_user_hash TEXT NOT NULL,
      display_name TEXT,
      created_at INTEGER NOT NULL,
      status TEXT NOT NULL,
      UNIQUE(provider, provider_user_hash)
    );
    CREATE TABLE IF NOT EXISTS cloud_devices (
      id TEXT PRIMARY KEY,
      account_id TEXT NOT NULL,
      name TEXT NOT NULL,
      platform TEXT NOT NULL,
      version TEXT,
      public_key TEXT NOT NULL,
      relay_tunnel_id TEXT NOT NULL UNIQUE,
      created_at INTEGER NOT NULL,
      last_seen_at INTEGER,
      revoked_at INTEGER,
      FOREIGN KEY(account_id) REFERENCES cloud_accounts(id)
    );
    CREATE INDEX IF NOT EXISTS idx_cloud_devices_account ON cloud_devices(account_id);
    CREATE TABLE IF NOT EXISTS cloud_device_tokens (
      device_id TEXT PRIMARY KEY,
      token_hash TEXT NOT NULL,
      created_at INTEGER NOT NULL,
      last_used_at INTEGER,
      FOREIGN KEY(device_id) REFERENCES cloud_devices(id)
    );
    CREATE INDEX IF NOT EXISTS idx_cloud_device_tokens_hash
      ON cloud_device_tokens(token_hash);
    CREATE TABLE IF NOT EXISTS cloud_login_codes (
      code_hash TEXT PRIMARY KEY,
      created_at INTEGER NOT NULL,
      expires_at INTEGER NOT NULL,
      status TEXT NOT NULL,
      provider_user_hash TEXT,
      approved_at INTEGER
    );
    CREATE INDEX IF NOT EXISTS idx_cloud_login_codes_expiry
      ON cloud_login_codes(expires_at);
    CREATE TABLE IF NOT EXISTS cloud_bot_binding (
      id INTEGER PRIMARY KEY CHECK (id = 1),
      provider TEXT NOT NULL,
      provider_user_hash TEXT NOT NULL,
      bound_at INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS cloud_bot_config (
      id INTEGER PRIMARY KEY CHECK (id = 1),
      token_ciphertext TEXT NOT NULL,
      token_iv TEXT NOT NULL,
      token_tag TEXT NOT NULL,
      allowed_user_hash TEXT,
      updated_at INTEGER NOT NULL
    );
  `);
  const existing = db
    .prepare("SELECT version FROM cloud_schema_migration WHERE version = 1")
    .get();
  if (!existing) {
    db.prepare(
      "INSERT INTO cloud_schema_migration (version, applied_at, checksum) VALUES (1, ?, ?)",
    ).run(Date.now(), "mode-cloud-v1");
  }
}
