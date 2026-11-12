import { createHmac } from "node:crypto";

import type { IceServerPayload } from "@mode/remote-core";

/** WebRTC 选路配置：STUN 公共可用；TURN 支持 coturn REST 与 Cloudflare TURN 两种来源。 */
export interface WebRtcConfig {
  stunUrls?: string[];
  turn?: {
    urls: string[];
    /** coturn `static-auth-secret`。 */
    secret: string;
    /** 凭证有效期（秒）；缺省 3600。 */
    ttlSeconds?: number;
  };
  /**
   * Cloudflare Realtime TURN：用 key id + API token 换取短期凭证。
   * NAS 没有公网 IP，自建 coturn 无法被外网访问；跨 NAT 走 Cloudflare 中转。
   */
  cloudflareTurn?: {
    keyId: string;
    /** TURN key 的 secret（创建 key 时一次性返回），只保存在服务端。 */
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
  extraServers: IceServerPayload[] = [],
): IceServerPayload[] | undefined {
  if (!config) return extraServers.length > 0 ? extraServers : undefined;
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
  servers.push(...extraServers);
  return servers.length > 0 ? servers : undefined;
}

const CLOUDFLARE_TURN_ENDPOINT = "https://rtc.live.cloudflare.com/v1/turn/keys";

/**
 * 宽松解析 Cloudflare 返回的 iceServers：官方响应是**单个对象**
 * `{urls, username, credential}`，也兼容数组形式；空 url 条目丢弃。
 */
function parseCloudflareIceServers(value: unknown): IceServerPayload[] {
  const entries = Array.isArray(value) ? value : value ? [value] : [];
  const servers: IceServerPayload[] = [];
  for (const entry of entries) {
    if (!entry || typeof entry !== "object") continue;
    const record = entry as { urls?: unknown; username?: unknown; credential?: unknown };
    const urls = Array.isArray(record.urls)
      ? record.urls.filter((url): url is string => typeof url === "string" && url.length > 0)
      : typeof record.urls === "string" && record.urls.length > 0
        ? [record.urls]
        : [];
    if (urls.length === 0) continue;
    servers.push({
      urls,
      ...(typeof record.username === "string" ? { username: record.username } : {}),
      ...(typeof record.credential === "string" ? { credential: record.credential } : {}),
    });
  }
  return servers;
}

/** 向 Cloudflare 换取短期 TURN 凭证；失败抛错，由缓存层决定是否沿用旧凭证。 */
export async function fetchCloudflareTurnIceServers(
  config: NonNullable<WebRtcConfig["cloudflareTurn"]>,
  options: { fetchImpl?: typeof fetch } = {},
): Promise<IceServerPayload[]> {
  const fetchImpl = options.fetchImpl ?? fetch;
  const response = await fetchImpl(
    `${CLOUDFLARE_TURN_ENDPOINT}/${encodeURIComponent(config.keyId)}/credentials/generate`,
    {
      method: "POST",
      headers: {
        authorization: `Bearer ${config.secret}`,
        "content-type": "application/json",
      },
      body: JSON.stringify({ ttl: Math.max(60, config.ttlSeconds ?? 3600) }),
    },
  );
  if (!response.ok) {
    throw new Error(`Cloudflare TURN 凭证请求失败 HTTP ${response.status}`);
  }
  const body = (await response.json()) as { iceServers?: unknown };
  const servers = parseCloudflareIceServers(body.iceServers);
  if (servers.length === 0) {
    throw new Error("Cloudflare TURN 凭证响应缺少 iceServers");
  }
  return servers;
}

interface CloudflareTurnCache {
  servers: IceServerPayload[];
  expiresAt: number;
}

/**
 * ICE 服务器提供者：connect.* 的调用路径是同步的，Cloudflare 凭证必须在后台预热。
 * 刷新失败时保留仍在有效期内的旧凭证；彻底没有时只下发 STUN/coturn，客户端照常回落 Relay。
 */
export class IceServerProvider {
  private cache: CloudflareTurnCache | null = null;
  private refreshing: Promise<void> | null = null;
  private readonly fetchImpl: typeof fetch;
  private readonly now: () => number;
  private readonly log: (line: string) => void;

  constructor(
    private readonly config: WebRtcConfig | undefined,
    options: { fetchImpl?: typeof fetch; now?: () => number; log?: (line: string) => void } = {},
  ) {
    this.fetchImpl = options.fetchImpl ?? fetch;
    this.now = options.now ?? Date.now;
    this.log = options.log ?? (() => undefined);
  }

  /** 启动时调用：拿到首批凭证再对外服务（失败只记日志，不阻断启动）。 */
  async warmup(): Promise<void> {
    if (!this.config?.cloudflareTurn) return;
    await this.refresh();
  }

  /** 并发调用共享同一在途请求。 */
  async refresh(): Promise<void> {
    const config = this.config?.cloudflareTurn;
    if (!config) return;
    if (this.refreshing) return this.refreshing;
    this.refreshing = (async () => {
      try {
        const servers = await fetchCloudflareTurnIceServers(config, { fetchImpl: this.fetchImpl });
        const ttlSeconds = Math.max(60, config.ttlSeconds ?? 3600);
        this.cache = { servers, expiresAt: this.now() + ttlSeconds * 1000 };
        this.log(
          `ice_cloudflare_turn=enabled urls=${servers.flatMap((server) => server.urls).join(",")}`,
        );
      } catch (error) {
        this.log(
          `ice_cloudflare_turn_failed error=${error instanceof Error ? error.message : String(error)}`,
        );
      } finally {
        this.refreshing = null;
      }
    })();
    return this.refreshing;
  }

  /** 定时器回调：临近过期（<60s）才真正发请求。 */
  maybeRefresh(): void {
    if (!this.config?.cloudflareTurn) return;
    const cache = this.cache;
    if (cache && cache.expiresAt - 60_000 > this.now()) return;
    void this.refresh();
  }

  get(deviceId: string): IceServerPayload[] | undefined {
    const cache = this.cache;
    const cloudflare = cache && cache.expiresAt > this.now() ? cache.servers : [];
    return buildIceServers(this.config, deviceId, cloudflare);
  }
}
