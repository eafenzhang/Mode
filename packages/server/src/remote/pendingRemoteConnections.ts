/**
 * Web 主控发起 SSH/WSL/Docker 连接后、浏览器接管 `/ws/remote/:id` 之前的暂存区
 * （docs/specs/web-remote-workspace-ssh-docker.md §4）。
 *
 * 旧实现是一个模块级 Map：POST 成功后浏览器不接管，连接（SSH 会话 + 远端进程）
 * 会永久留在内存里。这里补三件事：60s TTL、并发上限、服务关闭时全量释放。
 * 连接本身仍由 `/ws/remote/:id` 的 close 释放，接管即从本表删除。
 */
import { randomUUID } from "node:crypto";
import type { RemoteConnection } from "./connect.js";

export const PENDING_REMOTE_TTL_MS = 60_000;
export const PENDING_REMOTE_MAX = 4;

export type AddPendingRemoteResult =
  | { ok: true; id: string }
  | { ok: false; error: "too_many_pending_connections" };

export interface PendingRemoteConnectionsOptions {
  ttlMs?: number;
  maxPending?: number;
  idFactory?: () => string;
  log?: (...args: unknown[]) => void;
}

export class PendingRemoteConnections {
  private readonly entries = new Map<
    string,
    { connection: RemoteConnection; timer: ReturnType<typeof setTimeout> }
  >();

  constructor(private readonly options: PendingRemoteConnectionsOptions = {}) {}

  add(connection: RemoteConnection): AddPendingRemoteResult {
    const maxPending = this.options.maxPending ?? PENDING_REMOTE_MAX;
    if (this.entries.size >= maxPending) {
      return { ok: false, error: "too_many_pending_connections" };
    }
    const id = (this.options.idFactory ?? randomUUID)();
    const timer = setTimeout(() => {
      const entry = this.entries.get(id);
      if (!entry) {
        return;
      }
      this.entries.delete(id);
      this.options.log?.(`[remote] pending connection expired: ${id}`);
      entry.connection.dispose();
    }, this.options.ttlMs ?? PENDING_REMOTE_TTL_MS);
    timer.unref();
    this.entries.set(id, { connection, timer });
    return { ok: true, id };
  }

  /** 浏览器接管：取出后即从本表删除，后续由 WS close 负责释放。 */
  take(id: string): RemoteConnection | null {
    const entry = this.entries.get(id);
    if (!entry) {
      return null;
    }
    clearTimeout(entry.timer);
    this.entries.delete(id);
    return entry.connection;
  }

  disposeAll(): void {
    for (const entry of this.entries.values()) {
      clearTimeout(entry.timer);
      entry.connection.dispose();
    }
    this.entries.clear();
  }

  get size(): number {
    return this.entries.size;
  }
}
