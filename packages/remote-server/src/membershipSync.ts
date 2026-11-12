import { randomUUID } from "node:crypto";
import type { DatabaseSync } from "node:sqlite";

import type { DeviceRow } from "./storeTypes.js";

/**
 * 同账号设备自动建立 membership（个人账号体系）。
 *
 * 从 CloudStore 拆出只是为了让 store.ts 保持在文件行数门禁内；语义与事务边界仍由
 * 调用方（设备注册事务）决定。
 */
export function syncMemberships(db: DatabaseSync, accountId: string, deviceId: string): void {
  const device = db.prepare("SELECT * FROM devices WHERE id = ?").get(deviceId) as
    | DeviceRow
    | undefined;
  if (!device) return;
  const oppositeRole = device.role === "host" ? "client" : "host";
  const peers = db
    .prepare(
      "SELECT id FROM devices WHERE account_id = ? AND role = ? AND id <> ? AND revoked_at IS NULL",
    )
    .all(accountId, oppositeRole, deviceId) as Array<{ id: string }>;
  const hostId = device.role === "host" ? deviceId : null;
  const clientId = device.role === "client" ? deviceId : null;
  for (const peer of peers) {
    const host = hostId ?? peer.id;
    const client = clientId ?? peer.id;
    const existing = db
      .prepare("SELECT id FROM memberships WHERE host_device_id = ? AND client_device_id = ?")
      .get(host, client) as { id: string } | undefined;
    if (existing) {
      db.prepare("UPDATE memberships SET revoked_at = NULL WHERE id = ?").run(existing.id);
    } else {
      db.prepare(
        "INSERT INTO memberships (id, host_device_id, client_device_id, created_at) VALUES (?, ?, ?, ?)",
      ).run(randomUUID(), host, client, Date.now());
    }
  }
}
