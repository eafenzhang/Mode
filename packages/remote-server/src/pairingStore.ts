import { randomInt, randomUUID } from "node:crypto";
import type { DatabaseSync } from "node:sqlite";

import { randomToken, safeEqualHex, sha256Hex } from "./crypto.js";
import {
  StoreError,
  type AccountRow,
  type DeviceCredential,
  type DeviceRow,
  type PairingRow,
} from "./storeTypes.js";

/** 配对验证码允许的错误次数；达到即作废会话（docs/specs/remote-control-v3-pairing.md）。 */
const MAX_PAIRING_ATTEMPTS = 5;

export interface PairingStoreDeps {
  findDevice(deviceId: string): DeviceRow | null;
  createOrFindAccount(provider: string, anchor: string, displayName?: string): AccountRow;
  issueDeviceCredentials(deviceId: string): DeviceCredential;
}

/**
 * 跨账号配对会话存储。
 *
 * 与 `CloudStore` 共用同一个 SQLite 连接（设备/账号/凭证的唯一事实源在那边），
 * 独立成类只是为了控制文件体量与职责边界；状态机与事务都在这里。
 */
export class PairingStore {
  constructor(
    private readonly db: DatabaseSync,
    private readonly deps: PairingStoreDeps,
  ) {}

  findPairingSession(id: string): PairingRow | null {
    return (
      (this.db.prepare("SELECT * FROM pairing_sessions WHERE id = ?").get(id) as
        | PairingRow
        | undefined) ?? null
    );
  }

  findPairingByTicket(ticket: string): PairingRow | null {
    return (
      (this.db
        .prepare("SELECT * FROM pairing_sessions WHERE ticket_hash = ?")
        .get(sha256Hex(ticket)) as PairingRow | undefined) ?? null
    );
  }

  createPairingSession(hostDeviceId: string, ttlMs: number): { pairing: PairingRow; ticket: string } {
    const host = this.deps.findDevice(hostDeviceId);
    if (!host || host.revoked_at !== null) {
      throw new StoreError("device_not_found", "设备不存在");
    }
    if (host.role !== "host") {
      throw new StoreError("pairing_role_invalid", "只有 Host 能创建配对会话");
    }
    const id = randomUUID();
    const ticket = randomToken();
    const now = Date.now();
    this.db
      .prepare(
        `INSERT INTO pairing_sessions
         (id, ticket_hash, host_device_id, client_device_id, code_hash, status, created_at, expires_at, claimed_at, confirmed_at, redeemed_at, attempts)
         VALUES (?, ?, ?, NULL, NULL, 'pending', ?, ?, NULL, NULL, NULL, 0)`,
      )
      .run(id, sha256Hex(ticket), hostDeviceId, now, now + ttlMs);
    return { pairing: this.findPairingSession(id)!, ticket };
  }

  claimPairingSession(input: { ticket: string; clientDeviceId: string }): {
    pairing: PairingRow;
    code: string;
  } {
    const client = this.deps.findDevice(input.clientDeviceId);
    if (!client || client.revoked_at !== null) {
      throw new StoreError("device_not_found", "设备不存在");
    }
    if (client.role !== "client") {
      throw new StoreError("pairing_role_invalid", "只有 Client 能认领配对会话");
    }
    return this.transaction(() => {
      const pairing = this.findPairingByTicket(input.ticket);
      if (!pairing) throw new StoreError("pairing_invalid_ticket", "配对票无效");
      if (pairing.status !== "pending") {
        throw new StoreError("pairing_already_used", "配对会话已被使用");
      }
      if (Date.now() > pairing.expires_at) {
        throw new StoreError("pairing_expired", "配对会话已过期");
      }
      if (pairing.host_device_id === client.id) {
        throw new StoreError("pairing_invalid_ticket", "不能与自己配对");
      }
      const host = this.deps.findDevice(pairing.host_device_id);
      if (!host || host.revoked_at !== null) {
        throw new StoreError("pairing_invalid_ticket", "Host 不存在");
      }
      if (host.account_id === client.account_id) {
        throw new StoreError("same_account", "同账号设备请直接使用设备目录");
      }
      const code = randomInt(0, 100_000_000).toString().padStart(8, "0");
      this.db
        .prepare(
          `UPDATE pairing_sessions
           SET client_device_id = ?, code_hash = ?, status = 'claimed', claimed_at = ?, attempts = 0
           WHERE id = ? AND status = 'pending'`,
        )
        .run(client.id, sha256Hex(code), Date.now(), pairing.id);
      return { pairing: this.findPairingSession(pairing.id)!, code };
    });
  }

  confirmPairingSession(input: {
    id: string;
    hostDeviceId: string;
    code: string;
  }): { pairing: PairingRow; credentials: DeviceCredential } {
    const pairing = this.findPairingSession(input.id);
    if (!pairing || pairing.host_device_id !== input.hostDeviceId) {
      throw new StoreError("pairing_invalid_ticket", "配对会话不存在");
    }
    if (pairing.status === "confirmed" || pairing.status === "canceled") {
      throw new StoreError("pairing_already_used", "配对会话已结束");
    }
    if (pairing.status !== "claimed" || !pairing.client_device_id) {
      throw new StoreError("pairing_not_claimed", "对方尚未确认配对");
    }
    if (Date.now() > pairing.expires_at) {
      throw new StoreError("pairing_expired", "配对会话已过期");
    }
    if (!pairing.code_hash || !safeEqualHex(pairing.code_hash, sha256Hex(input.code))) {
      // 计数更新必须在事务外提交：错误码路径要留下次数，否则回滚后可以无限重试。
      const attempts = pairing.attempts + 1;
      if (attempts >= MAX_PAIRING_ATTEMPTS) {
        this.db
          .prepare("UPDATE pairing_sessions SET attempts = ?, status = 'canceled' WHERE id = ?")
          .run(attempts, pairing.id);
        throw new StoreError("pairing_attempts_exhausted", "验证码错误次数过多，会话已作废");
      }
      this.db
        .prepare("UPDATE pairing_sessions SET attempts = ? WHERE id = ?")
        .run(attempts, pairing.id);
      throw new StoreError("pairing_code_invalid", "验证码错误");
    }

    return this.transaction(() => {
      // 事务内复核状态：并发 confirm 只有一个能把 claimed 推到 confirmed。
      const current = this.findPairingSession(input.id);
      if (!current || current.status !== "claimed" || !current.client_device_id) {
        throw new StoreError("pairing_already_used", "配对会话已结束");
      }
      const account = this.deps.createOrFindAccount(
        "device",
        sha256Hex(`pairing:${current.id}`),
        "设备配对账号",
      );
      const now = Date.now();
      const clientId = current.client_device_id;
      // 两台设备迁入 device_only 账号：Host 只能被连、Client 无 Web/Admin 能力。
      this.db
        .prepare("UPDATE devices SET account_id = ?, inbound_only = 1 WHERE id = ?")
        .run(account.id, current.host_device_id);
      this.db
        .prepare("UPDATE devices SET account_id = ?, pairing_restricted = 1 WHERE id = ?")
        .run(account.id, clientId);
      // 旧账号的 membership 全部作废，只保留这一条配对关系。
      this.db
        .prepare(
          `UPDATE memberships SET revoked_at = ?
           WHERE revoked_at IS NULL
             AND (host_device_id IN (?, ?) OR client_device_id IN (?, ?))`,
        )
        .run(now, current.host_device_id, clientId, current.host_device_id, clientId);
      this.db
        .prepare(
          "INSERT INTO memberships (id, host_device_id, client_device_id, created_at) VALUES (?, ?, ?, ?)",
        )
        .run(randomUUID(), current.host_device_id, clientId, now);
      // Host 立即换新凭证；Client 在 redeem 时换（旧 token 用于认证那一次领取）。
      const credentials = this.deps.issueDeviceCredentials(current.host_device_id);
      this.db
        .prepare("UPDATE pairing_sessions SET status = 'confirmed', confirmed_at = ? WHERE id = ?")
        .run(now, current.id);
      return { pairing: this.findPairingSession(current.id)!, credentials };
    });
  }

  redeemPairingSession(input: { id: string; clientDeviceId: string }): DeviceCredential {
    return this.transaction(() => {
      const pairing = this.findPairingSession(input.id);
      if (!pairing || pairing.client_device_id !== input.clientDeviceId) {
        throw new StoreError("pairing_invalid_ticket", "配对会话不存在");
      }
      if (pairing.status !== "confirmed") {
        throw new StoreError("pairing_not_confirmed", "Host 尚未确认配对");
      }
      if (pairing.redeemed_at !== null) {
        throw new StoreError("pairing_already_used", "设备凭证已领取");
      }
      const credentials = this.deps.issueDeviceCredentials(input.clientDeviceId);
      this.db
        .prepare("UPDATE pairing_sessions SET redeemed_at = ? WHERE id = ?")
        .run(Date.now(), pairing.id);
      return credentials;
    });
  }

  cancelPairingSession(input: { id: string; deviceId: string }): PairingRow {
    return this.transaction(() => {
      const pairing = this.findPairingSession(input.id);
      if (!pairing) throw new StoreError("pairing_invalid_ticket", "配对会话不存在");
      if (pairing.host_device_id !== input.deviceId && pairing.client_device_id !== input.deviceId) {
        throw new StoreError("forbidden", "无权取消该配对会话");
      }
      if (pairing.status === "confirmed") {
        throw new StoreError("pairing_already_used", "配对已完成");
      }
      this.db
        .prepare(
          "UPDATE pairing_sessions SET status = 'canceled' WHERE id = ? AND status IN ('pending','claimed')",
        )
        .run(pairing.id);
      return this.findPairingSession(pairing.id)!;
    });
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
