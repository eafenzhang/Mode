/**
 * Mode Cloud SQLite schema。从 cloudStore 抽出以保持单文件行数限制。
 *
 * v3（docs/specs/cloud-personal-accounts.md，GitHub-only v2）：
 * - cloud_devices 增加 access_token_cipher（AES-256-GCM 密文）与 addresses（JSON 数组）；
 * - cloud_accounts 删除 username / password_hash（密码注册已移除；DROP COLUMN 失败时
 *   保留为无害死列，代码不再读写）。
 */
import type { DatabaseSync } from "node:sqlite";

function addColumnIfMissing(
  db: DatabaseSync,
  table: string,
  column: string,
  declaration: string,
): void {
  const columns = db.prepare(`PRAGMA table_info(${table})`).all() as Array<{
    name?: unknown;
  }>;
  const exists = columns.some((entry) => entry.name === column);
  if (!exists) {
    db.exec(`ALTER TABLE ${table} ADD COLUMN ${column} ${declaration}`);
  }
}

function dropColumnIfPresent(db: DatabaseSync, table: string, column: string): void {
  const columns = db.prepare(`PRAGMA table_info(${table})`).all() as Array<{
    name?: unknown;
  }>;
  if (!columns.some((entry) => entry.name === column)) {
    return;
  }
  try {
    db.exec(`ALTER TABLE ${table} DROP COLUMN ${column}`);
  } catch {
    // 旧 SQLite 不支持 DROP COLUMN：保留死列，代码层不再读写。
  }
}

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
    CREATE TABLE IF NOT EXISTS cloud_user_sessions (
      id TEXT PRIMARY KEY,
      account_id TEXT NOT NULL,
      token_hash TEXT NOT NULL UNIQUE,
      client_label TEXT,
      created_at INTEGER NOT NULL,
      last_used_at INTEGER,
      expires_at INTEGER NOT NULL,
      revoked_at INTEGER,
      FOREIGN KEY(account_id) REFERENCES cloud_accounts(id)
    );
    CREATE INDEX IF NOT EXISTS idx_cloud_user_sessions_account
      ON cloud_user_sessions(account_id);
    CREATE INDEX IF NOT EXISTS idx_cloud_user_sessions_expiry
      ON cloud_user_sessions(expires_at);
  `);
  // v3 列迁移：设备访问令牌密文 + 自报地址；密码列删除（GitHub-only）。
  addColumnIfMissing(db, "cloud_devices", "relay_token_hash", "TEXT");
  addColumnIfMissing(db, "cloud_devices", "access_token_cipher", "TEXT");
  addColumnIfMissing(db, "cloud_devices", "addresses", "TEXT");
  dropColumnIfPresent(db, "cloud_accounts", "username");
  dropColumnIfPresent(db, "cloud_accounts", "password_hash");
  dropColumnIfPresent(db, "cloud_accounts", "updated_at");
  const applied = db
    .prepare("SELECT version FROM cloud_schema_migration ORDER BY version DESC LIMIT 1")
    .get() as { version?: unknown } | undefined;
  const currentVersion = typeof applied?.version === "number" ? applied.version : 0;
  if (currentVersion < 3) {
    db.prepare(
      "INSERT INTO cloud_schema_migration (version, applied_at, checksum) VALUES (?, ?, ?)",
    ).run(3, Date.now(), "mode-cloud-v3-github-only");
  }
}
