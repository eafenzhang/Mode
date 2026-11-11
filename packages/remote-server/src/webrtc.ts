import { createHmac } from "node:crypto";

import type { IceServerPayload } from "@mode/remote-core";

/** WebRTC 选路配置：STUN 公共可用；TURN 用 coturn REST 临时凭证（`use-auth-secret`）。 */
export interface WebRtcConfig {
  stunUrls?: string[];
  turn?: {
    urls: string[];
    /** coturn `static-auth-secret`。 */
    secret: string;
    /** 凭证有效期（秒）；缺省 3600。 */
    ttlSeconds?: number;
  };
}

/** coturn REST API 临时凭证：username = `<过期时间戳>:<deviceId>`，credential = HMAC-SHA1。 */
export function buildTurnCredentials(
  secret: string,
  ttlSeconds: number,
  deviceId: string,
  nowSeconds = Math.floor(Date.now() / 1000),
): { username: string; credential: string } {
  const ttl = Math.max(60, ttlSeconds);
  const username = `${nowSeconds + ttl}:${deviceId}`;
  const credential = createHmac("sha1", secret).update(username).digest("base64");
  return { username, credential };
}

/** 组装 connect.* 下发的 ICE 服务器列表；未配置或全空时返回 undefined。 */
export function buildIceServers(
  config: WebRtcConfig | undefined,
  deviceId: string,
): IceServerPayload[] | undefined {
  if (!config) return undefined;
  const servers: IceServerPayload[] = [];
  if (config.stunUrls && config.stunUrls.length > 0) {
    servers.push({ urls: config.stunUrls });
  }
  if (config.turn && config.turn.urls.length > 0 && config.turn.secret) {
    const { username, credential } = buildTurnCredentials(
      config.turn.secret,
      config.turn.ttlSeconds ?? 3600,
      deviceId,
    );
    servers.push({ urls: config.turn.urls, username, credential });
  }
  return servers.length > 0 ? servers : undefined;
}
