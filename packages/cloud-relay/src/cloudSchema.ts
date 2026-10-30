/**
 * Mode Cloud SQLite schema。从 cloudStore 抽出以保持单文件行数限制。
 *
 * v2（docs/specs/cloud-personal-accounts.md）：
 * - cloud_accounts 增加 username / password_hash / updated_at：个人账号（用户名+密码）。
 * - cloud_user_sessions：人类主控会话（sessionToken 只存 SHA-256）。
 * - cloud_devices 增加 relay_token_hash：每设备中继令牌（替换部署级共享 secret）。
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
  // v2 列迁移：老库逐列补齐（新库由上面的 CREATE TABLE 建出后再 ALTER 会重复添加，
  // 因此必须按 PRAGMA 判断）。新列可空：存量微信锚点账号在补设用户名密码前保持 NULL。
  addColumnIfMissing(db, "cloud_accounts", "username", "TEXT");
  addColumnIfMissing(db, "cloud_accounts", "password_hash", "TEXT");
  addColumnIfMissing(db, "cloud_accounts", "updated_at", "INTEGER");
  addColumnIfMissing(db, "cloud_devices", "relay_token_hash", "TEXT");
  const applied = db
    .prepare("SELECT version FROM cloud_schema_migration ORDER BY version DESC LIMIT 1")
    .get() as { version?: unknown } | undefined;
  const currentVersion = typeof applied?.version === "number" ? applied.version : 0;
  if (currentVersion < 2) {
    db.prepare(
      "INSERT INTO cloud_schema_migration (version, applied_at, checksum) VALUES (?, ?, ?)",
    ).run(2, Date.now(), "mode-cloud-v2-personal-accounts");
  }
}
