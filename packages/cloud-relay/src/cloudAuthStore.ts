/**
 * 个人账号的凭据存储（docs/specs/cloud-personal-accounts.md）。
 *
 * 与 CloudStore 共享同一个 SQLite 连接（唯一持久化真相仍是这个库）；
 * 单独成类是为了让 cloudStore.ts / 本文件都低于 400 行 lint 上限。
 * 只存用户名明文（非凭据）、密码哈希与会话令牌哈希；原文令牌只在签发响应出现一次。
 */
import { randomUUID } from "node:crypto";

import type { DatabaseSync } from "node:sqlite";

export interface CloudAccountRow {
  id: string;
  provider: string;
  providerUserHash: string;
  username: string | null;
  displayName: string | null;
  /** scrypt 串；微信锚点账号在补设密码前为 null。 */
  passwordHash: string | null;
  status: string;
}

export interface CloudSessionRow {
  id: string;
  accountId: string;
  clientLabel: string | null;
  createdAt: number;
  lastUsedAt: number | null;
  expiresAt: number;
}

export function mapAuthAccount(row: Record<string, unknown>): CloudAccountRow {
  return {
    id: String(row.id ?? ""),
    provider: String(row.provider ?? ""),
    providerUserHash: String(row.provider_user_hash ?? ""),
    username: typeof row.username === "string" && row.username ? row.username : null,
    displayName: typeof row.display_name === "string" ? row.display_name : null,
    passwordHash:
      typeof row.password_hash === "string" && row.password_hash ? row.password_hash : null,
    status: String(row.status ?? ""),
  };
}

/** 保留用户名：不能被注册为个人账号。 */
const RESERVED_USERNAMES = new Set([
  "admin",
  "administrator",
  "cloud",
  "mode",
  "null",
  "root",
  "support",
  "system",
]);

export function validateUsername(raw: string): string | null {
  const username = raw.trim().toLowerCase();
  if (!/^[a-z0-9_-]{3,32}$/u.test(username) || RESERVED_USERNAMES.has(username)) {
    return null;
  }
  return username;
}

export function validatePassword(raw: string): boolean {
  return raw.length >= 8 && raw.length <= 128;
}

export class CloudAuthStore {
  constructor(private readonly db: DatabaseSync) {}

  /** 创建账号（provider='password' 密码注册 / provider='github' JIT 建号）。 */
  createAccount(input: {
    provider: string;
    providerUserHash: string;
    username?: string;
    passwordHash?: string;
    displayName?: string;
    now: number;
  }): CloudAccountRow | "conflict" {
    const existing = this.db
      .prepare(
        "SELECT * FROM cloud_accounts WHERE provider = ? AND provider_user_hash = ?",
      )
      .get(input.provider, input.providerUserHash) as Record<string, unknown> | undefined;
    if (existing) {
      return "conflict";
    }
    const id = randomUUID();
    this.db
      .prepare(
        "INSERT INTO cloud_accounts (id, provider, provider_user_hash, username, password_hash, display_name, created_at, status) VALUES (?, ?, ?, ?, ?, ?, ?, 'active')",
      )
      .run(
        id,
        input.provider,
        input.providerUserHash,
        input.username ?? null,
        input.passwordHash ?? null,
        input.displayName ?? null,
        input.now,
      );
    return mapAuthAccount({
      id,
      provider: input.provider,
      provider_user_hash: input.providerUserHash,
      username: input.username ?? null,
      password_hash: input.passwordHash ?? null,
      display_name: input.displayName ?? null,
      status: "active",
    });
  }

  /** 密码登录按 username 列查找（GitHub/迁移账号补设密码后同样命中）。 */
  findAccountByUsername(username: string): CloudAccountRow | null {
    const row = this.db
      .prepare("SELECT * FROM cloud_accounts WHERE username = ?")
      .get(username) as Record<string, unknown> | undefined;
    return row ? mapAuthAccount(row) : null;
  }

  findAccountByAnchor(provider: string, providerUserHash: string): CloudAccountRow | null {
    const row = this.db
      .prepare("SELECT * FROM cloud_accounts WHERE provider = ? AND provider_user_hash = ?")
      .get(provider, providerUserHash) as Record<string, unknown> | undefined;
    return row ? mapAuthAccount(row) : null;
  }

  findAccountById(id: string): CloudAccountRow | null {
    const row = this.db
      .prepare("SELECT * FROM cloud_accounts WHERE id = ?")
      .get(id) as Record<string, unknown> | undefined;
    return row ? mapAuthAccount(row) : null;
  }

  /** 微信/GitHub 锚点账号补设用户名密码；用户名被占用返回 conflict。 */
  setAccountCredentials(input: {
    accountId: string;
    username: string;
    usernameHash: string;
    passwordHash: string;
    now: number;
  }): "ok" | "conflict" | "not_found" {
    const taken = this.db
      .prepare(
        "SELECT id FROM cloud_accounts WHERE provider = 'password' AND provider_user_hash = ? AND id <> ?",
      )
      .get(input.usernameHash, input.accountId) as Record<string, unknown> | undefined;
    if (taken) {
      return "conflict";
    }
    const result = this.db
      .prepare(
        "UPDATE cloud_accounts SET username = ?, password_hash = ?, updated_at = ? WHERE id = ?",
      )
      .run(input.username, input.passwordHash, input.now, input.accountId);
    return Number(result.changes ?? 0) > 0 ? "ok" : "not_found";
  }

  createSession(input: {
    accountId: string;
    tokenHash: string;
    clientLabel?: string;
    now: number;
    expiresAt: number;
  }): CloudSessionRow {
    const id = randomUUID();
    this.db
      .prepare(
        "INSERT INTO cloud_user_sessions (id, account_id, token_hash, client_label, created_at, last_used_at, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
      )
      .run(
        id,
        input.accountId,
        input.tokenHash,
        input.clientLabel ?? null,
        input.now,
        input.now,
        input.expiresAt,
      );
    // 过期 7 天后的会话行不再有展示/安全价值，顺手清理防止表无限增长。
    this.db
      .prepare("DELETE FROM cloud_user_sessions WHERE expires_at < ?")
      .run(input.now - 7 * 24 * 3_600_000);
    return {
      id,
      accountId: input.accountId,
      clientLabel: input.clientLabel ?? null,
      createdAt: input.now,
      lastUsedAt: input.now,
      expiresAt: input.expiresAt,
    };
  }

  findSessionByTokenHash(tokenHash: string, now: number): CloudSessionRow | null {
    const row = this.db
      .prepare(
        "SELECT * FROM cloud_user_sessions WHERE token_hash = ? AND revoked_at IS NULL AND expires_at > ?",
      )
      .get(tokenHash, now) as Record<string, unknown> | undefined;
    if (!row) {
      return null;
    }
    this.db
      .prepare("UPDATE cloud_user_sessions SET last_used_at = ? WHERE id = ?")
      .run(now, String(row.id));
    return {
      id: String(row.id),
      accountId: String(row.account_id),
      clientLabel: typeof row.client_label === "string" ? row.client_label : null,
      createdAt: Number(row.created_at ?? 0),
      lastUsedAt: Number(row.last_used_at ?? now),
      expiresAt: Number(row.expires_at ?? 0),
    };
  }

  /** refresh 轮转：旧会话立即失效，新会话同事务写入；失败（旧会话已失效）返回 null。 */
  rotateSession(input: {
    oldSessionId: string;
    accountId: string;
    tokenHash: string;
    clientLabel?: string;
    now: number;
    expiresAt: number;
  }): CloudSessionRow | null {
    this.db.exec("BEGIN IMMEDIATE");
    try {
      const revoked = this.db
        .prepare(
          "UPDATE cloud_user_sessions SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL AND expires_at > ?",
        )
        .run(input.now, input.oldSessionId, input.now);
      if (Number(revoked.changes ?? 0) === 0) {
        this.db.exec("ROLLBACK");
        return null;
      }
      const id = randomUUID();
      this.db
        .prepare(
          "INSERT INTO cloud_user_sessions (id, account_id, token_hash, client_label, created_at, last_used_at, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .run(
          id,
          input.accountId,
          input.tokenHash,
          input.clientLabel ?? null,
          input.now,
          input.now,
          input.expiresAt,
        );
      this.db.exec("COMMIT");
      return {
        id,
        accountId: input.accountId,
        clientLabel: input.clientLabel ?? null,
        createdAt: input.now,
        lastUsedAt: input.now,
        expiresAt: input.expiresAt,
      };
    } catch (error) {
      this.db.exec("ROLLBACK");
      throw error;
    }
  }

  revokeSessionByTokenHash(tokenHash: string): void {
    this.db
      .prepare(
        "UPDATE cloud_user_sessions SET revoked_at = ? WHERE token_hash = ? AND revoked_at IS NULL",
      )
      .run(Date.now(), tokenHash);
  }

  revokeSessionById(sessionId: string, accountId: string): boolean {
    const result = this.db
      .prepare(
        "UPDATE cloud_user_sessions SET revoked_at = ? WHERE id = ? AND account_id = ? AND revoked_at IS NULL",
      )
      .run(Date.now(), sessionId, accountId);
    return Number(result.changes ?? 0) > 0;
  }

  listSessions(accountId: string, now: number): CloudSessionRow[] {
    const rows = this.db
      .prepare(
        "SELECT * FROM cloud_user_sessions WHERE account_id = ? AND revoked_at IS NULL AND expires_at > ? ORDER BY created_at DESC",
      )
      .all(accountId, now) as Record<string, unknown>[];
    return rows.map((row) => ({
      id: String(row.id),
      accountId: String(row.account_id),
      clientLabel: typeof row.client_label === "string" ? row.client_label : null,
      createdAt: Number(row.created_at ?? 0),
      lastUsedAt: typeof row.last_used_at === "number" ? row.last_used_at : null,
      expiresAt: Number(row.expires_at ?? 0),
    }));
  }
}
