import { randomBytes } from "node:crypto";

export const DEFAULT_REMOTE_DESKTOP_TICKET_TTL_MS = 30_000;

export interface RemoteDesktopTicketStoreOptions {
  ttlMs?: number;
  now?: () => number;
  createTicket?: () => string;
}

export interface IssuedRemoteDesktopTicket {
  ticket: string;
  expiresAt: number;
}

export interface RemoteDesktopTicketStore {
  issue(clientId: string): IssuedRemoteDesktopTicket;
  consume(ticket: string | undefined, clientId: string | undefined): boolean;
  /**
   * media 端点专用：票据即凭证——原子消费后返回签发时绑定的 clientId，
   * 供升级中间件直接 c.set（调用方不再出示令牌）。未知/过期/空串一律作废并返回 null。
   */
  consumeAndResolveClientId(ticket: string | undefined): string | null;
}

/**
 * 一次性远程桌面会话票据（spec: docs/specs/lan-remote-desktop.md 状态所有者）。
 * 与 host capability 同一形态但独立实例：票据在签发时绑定 clientId，
 * 升级时必须由同一客户端出示才能消费，防止票据被其它已配对设备挪用。
 * 只在 HTTP server 进程内存中存在。
 */
export function createRemoteDesktopTicketStore(
  options: RemoteDesktopTicketStoreOptions = {},
): RemoteDesktopTicketStore {
  const ttlMs = options.ttlMs ?? DEFAULT_REMOTE_DESKTOP_TICKET_TTL_MS;
  const now = options.now ?? Date.now;
  const createTicket = options.createTicket ?? (() => randomBytes(32).toString("base64url"));
  const issuedByTicket = new Map<string, { clientId: string; expiresAt: number }>();

  const purgeExpired = (at: number): void => {
    for (const [ticket, issued] of issuedByTicket) {
      if (issued.expiresAt <= at) issuedByTicket.delete(ticket);
    }
  };

  return {
    issue(clientId) {
      const issuedAt = now();
      purgeExpired(issuedAt);
      const ticket = createTicket();
      const expiresAt = issuedAt + ttlMs;
      issuedByTicket.set(ticket, { clientId, expiresAt });
      return { ticket, expiresAt };
    },
    consume(ticket, clientId) {
      if (!ticket) return false;
      const consumedAt = now();
      const issued = issuedByTicket.get(ticket);
      // 与 host capability 相同的重放纪律：成功、过期、错客户端、重放都先删除再判定，
      // 保证一张票据最多只可能升级一次。
      issuedByTicket.delete(ticket);
      purgeExpired(consumedAt);
      if (!issued || issued.expiresAt <= consumedAt) return false;
      return Boolean(clientId) && issued.clientId === clientId;
    },
    consumeAndResolveClientId(ticket) {
      if (!ticket) return null;
      const consumedAt = now();
      const issued = issuedByTicket.get(ticket);
      // 重放纪律与 consume 完全同口径（先删后判）：一张票据最多只可能升级一次，
      // 区别仅在于不比对出示方——票据本身（令牌鉴权 POST 的产物）就是凭证。
      issuedByTicket.delete(ticket);
      purgeExpired(consumedAt);
      if (!issued || issued.expiresAt <= consumedAt) return null;
      return issued.clientId;
    },
  };
}
