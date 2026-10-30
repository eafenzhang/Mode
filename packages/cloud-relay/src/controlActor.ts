/**
 * 控制面主体解析与每设备中继令牌签发（docs/specs/cloud-personal-accounts.md）。
 *
 * 从 controlAuthPassword 拆出以保持行数上限；目录/revoke/credentials 共用
 * resolveControlActor 同时接受会话令牌（人类主控）与设备令牌（设备自查）。
 */
import { randomBytes } from "node:crypto";
import type { IncomingMessage } from "node:http";

import { hashDeviceToken, readBearer, readSessionCookie, type ControlApiOptions } from "./controlHttp.js";

const RELAY_TOKEN_BYTES = 32;

/** Bearer 会话优先、设备令牌兜底；返回账号与主体来源。 */
export function resolveControlActor(
  options: ControlApiOptions,
  req: IncomingMessage,
): {
  accountId: string;
  displayName: string | null;
  status: string;
  source: "session" | "device";
} | null {
  const now = options.now?.() ?? Date.now();
  // 门户走 HttpOnly Cookie，Desktop 走 Bearer；两者都映射到同一套会话记录。
  const bearer = readBearer(req) ?? readSessionCookie(req);
  if (bearer) {
    const session = options.store.auth.findSessionByTokenHash(hashDeviceToken(bearer), now);
    if (session) {
      const account = options.store.auth.findAccountById(session.accountId);
      if (account) {
        return {
          accountId: account.id,
          displayName: account.displayName,
          status: account.status,
          source: "session",
        };
      }
    }
    const found = options.store.findDeviceByTokenHash(hashDeviceToken(bearer));
    if (found) {
      return {
        accountId: found.account.id,
        displayName: found.account.displayName,
        status: found.account.status,
        source: "device",
      };
    }
  }
  return null;
}

/** 生成每设备中继令牌；哈希入库，原文只在 enroll 响应下发一次。 */
export function issueRelayToken(): string {
  return randomBytes(RELAY_TOKEN_BYTES).toString("base64url");
}
