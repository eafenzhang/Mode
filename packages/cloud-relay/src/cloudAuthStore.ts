/**
 * 个人账号的凭据存储（docs/specs/cloud-personal-accounts.md，v2）。
 *
 * 与 CloudStore 共享同一个 SQLite 连接（唯一持久化真相仍是这个库）；
 * 单独成类是为了让 cloudStore.ts / 本文件都低于 400 行 lint 上限。
 * 身份方式只有 GitHub：账号只存锚点哈希与显示名；令牌只存哈希，
 * 原文令牌只在签发响应出现一次。
 */
import { randomUUID } from "node:crypto";

import type { DatabaseSync } from "node:sqlite";

export interface CloudAccountRow {
  id: string;
  provider: string;
  providerUserHash: string;
  displayName: string | null;
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
    displayName: typeof row.display_name === "string" ? row.display_name : null,
    status: String(row.status ?? ""),
  };
}

export class CloudAuthStore {
  constructor(private readonly db: DatabaseSync) {}

  /** GitHub JIT 建号；同锚点已存在返回 conflict。 */
  createAccount(input: {
    providerUserHash: string;
    displayName?: string;
    now: number;
  }): CloudAccountRow | "conflict" {
    const existing = this.db
      .prepare("SELECT * FROM cloud_accounts WHERE provider = 'github' AND provider_user_hash = ?")
      .get(input.providerUserHash) as Record<string, unknown> | undefined;
    if (existing) {
      return "conflict";
    }
    const id = randomUUID();
    this.db
      .prepare(
        "INSERT INTO cloud_accounts (id, provider, provider_user_hash, display_name, created_at, status) VALUES (?, 'github', ?, ?, ?, 'active')",
      )
      .run(id, input.providerUserHash, input.displayName ?? null, input.now);
    return mapAuthAccount({
      id,
      provider: "github",
      provider_user_hash: input.providerUserHash,
      display_name: input.displayName ?? null,
      status: "active",
    });
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
