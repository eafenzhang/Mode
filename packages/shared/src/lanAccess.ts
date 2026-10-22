import { z } from "zod";
import { serverRemoteWorkspaceInfoSchema } from "./server-remote.js";

/**
 * 局域网访问契约：对端发现（UDP 单播应答）、配对（一次性配对码 → 长期令牌）、
 * 以及设置卡需要的状态快照。服务端（Mode 桌面端 Host）与客户端（远程连接向导）共用。
 *
 * 安全底线：发现应答只暴露「我是谁、在哪个端口、需要配对」，绝不含令牌；
 * 长期令牌只经配对接口下发一次，落盘走凭据服务。
 */

/** UDP 发现端口（固定，便于对端广播探测）。 */
export const LAN_ACCESS_DISCOVERY_PORT = 45879;
/** 探测报文；对端只对收到的探测单播应答，不做周期性广播。 */
export const LAN_ACCESS_PROBE = "mode-lan-discover-v1";
/** 发现/配对协议版本；不匹配时客户端应提示升级而不是硬连。 */
export const LAN_ACCESS_PROTOCOL_VERSION = 1;
/** 配对码有效期，与机器人绑定码一致。 */
export const LAN_ACCESS_PAIR_CODE_TTL_MS = 5 * 60_000;
/** 服务端首选监听端口；被占用时回落到系统分配的可用端口。 */
export const LAN_ACCESS_DEFAULT_PORT = 45880;
/**
 * 内置公网中继基础地址：主控不需要手填公网地址，被控端配对时自动下发
 * `https://<base>/d/<tunnelId>`。可用 `MODE_PUBLIC_RELAY_BASE_URL` 覆写。
 */
export const DEFAULT_PUBLIC_RELAY_BASE_URL = "https://relay.mymode.cloud";
/** 被控端持久化的中继路径 ID（tunnelId）；首次启用中继时生成。 */
export const LAN_ACCESS_RELAY_TUNNEL_ID_CREDENTIAL_KEY = "lan:access:relay-tunnel-id";

const lanTrimmedString = z.string().trim().min(1);

/** 服务端应答体（客户端据此展示设备名并决定是否需要配对）。 */
export const lanPeerAnnouncementSchema = z.object({
  magic: z.literal("mode-lan"),
  protocolVersion: z.literal(LAN_ACCESS_PROTOCOL_VERSION),
  /** 安装级稳定标识：用于历史条目、凭据键与「同一台设备」判定。 */
  serverId: lanTrimmedString,
  name: lanTrimmedString.optional(),
  version: z.string(),
  /** 对端 HTTP/WS 监听端口。 */
  port: z.number().int().positive().max(65535),
  requiresPairing: z.boolean(),
  platform: z.string().optional(),
});

export type LanPeerAnnouncement = z.infer<typeof lanPeerAnnouncementSchema>;

/** 客户端侧填充 host 后的发现结果。 */
export interface LanDiscoveredPeer extends LanPeerAnnouncement {
  host: string;
  /**
   * 同一 serverId+port 的其它可达地址（多网卡/多通道，如 LAN + Tailscale），
   * 端口与 host 相同；发现侧已按「主地址优先物理局域网」排序合并。
   */
  extraHosts?: string[];
}

export function buildLanProbePayload(): string {
  return LAN_ACCESS_PROBE;
}

export function buildLanAnnouncementPayload(
  info: Omit<LanPeerAnnouncement, "magic" | "protocolVersion">,
): string {
  return JSON.stringify({
    magic: "mode-lan",
    protocolVersion: LAN_ACCESS_PROTOCOL_VERSION,
    ...info,
  });
}

export function parseLanAnnouncement(text: string): LanPeerAnnouncement | null {
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    return null;
  }
  const parsed = lanPeerAnnouncementSchema.safeParse(raw);
  return parsed.success ? parsed.data : null;
}

/** 服务端已配对客户端记录（令牌本体只存凭据服务，列表只用于展示与撤销）。 */
export interface LanAccessClientRecord {
  id: string;
  label: string;
  createdAt: number;
  lastUsedAt: number | null;
  /** 该设备最近在本机打开过的工作区目录；老记录/从未打开时为 undefined，UI 显示「尚无」。 */
  lastWorkspacePath?: string | null;
  /**
   * 远程桌面逐设备授权（同意窗「记住此设备」/ 设置页开关写同一字段）：
   * 缺键 = 未授权（旧数据向后兼容），下次连接重新弹同意窗。
   */
  remoteDesktopAuthorized?: boolean;
}

/** 配对码（服务端设置卡展示，客户端输入）。 */
export interface LanAccessPairCode {
  code: string;
  expiresAt: number;
}

export const lanAccessClientRecordSchema = z
  .object({
    id: lanTrimmedString,
    label: z.string(),
    createdAt: z.number().int().nonnegative(),
    lastUsedAt: z.number().int().nonnegative().nullable(),
    // 可选以兼容改名前/未录制过的老记录。
    lastWorkspacePath: z.string().min(1).nullable().optional(),
    // strict schema 必须声明 host 记录会携带的全部字段：漏声明会让 main 的
    // hostResponseMessageSchema 整条拒收 lan-access-state-result（UI 表现为 8s 超时）。
    remoteDesktopAuthorized: z.boolean().optional(),
  })
  .strict();

export const lanAccessPairCodeSchema = z
  .object({
    code: z.string().trim().min(4).max(32),
    expiresAt: z.number().int().positive(),
  })
  .strict();

export const lanAccessStateSchema = z
  .object({
    enabled: z.boolean(),
    port: z.number().int().positive().max(65535).nullable(),
    addresses: z.array(z.string()),
    pairCode: lanAccessPairCodeSchema.nullable(),
    clients: z.array(lanAccessClientRecordSchema),
    // 当前在线的 web/手机远控会话数（已通过 LAN 令牌鉴权的 /ws 连接）。
    // 旧 Host 不返回该键；缺失按 0 处理。
    activeWebSessions: z.number().int().nonnegative().optional(),
    // 内置公网中继的公开地址；未启用/未配置时 null。旧 Host 不返回该键。
    publicRelayUrl: z.string().url().nullable().optional(),
  })
  .strict();

export interface LanAccessState {
  enabled: boolean;
  /** 未启用时为 null。 */
  port: number | null;
  /** 局域网可达地址（用于提示用户在客户端手动输入）。 */
  addresses: string[];
  /** 当前有效配对码；未启用或未生成时为 null。 */
  pairCode: LanAccessPairCode | null;
  clients: LanAccessClientRecord[];
  /** 当前在线的 web/手机远控会话数；旧 Host 缺失时按 0 处理。 */
  activeWebSessions?: number;
  /** 内置公网中继的公开地址；未启用/未配置时 null。 */
  publicRelayUrl?: string | null;
}

/** 服务端实现、packages/server 消费的配对端点契约（配对码 → 长期令牌）。 */
export interface LanAccessPairingEndpoint {
  redeem(request: LanAccessPairRequest): Promise<LanAccessPairResult | null>;
}

export const lanAccessPairRequestSchema = z.object({
  code: z.string().trim().min(4).max(32),
  label: z.string().trim().max(64).optional(),
});

export type LanAccessPairRequest = z.infer<typeof lanAccessPairRequestSchema>;

/** 服务端形态「允许局域网访问」开关请求体（设置页 ↔ packages/server 的 lanAccessHost）。 */
export const lanAccessEnableRequestSchema = z.object({ enabled: z.boolean() }).strict();

/** 被控端的内置公网中继信息：基础地址 + 本机持久化 tunnelId。 */
export interface LanPublicRelayInfo {
  baseUrl: string;
  tunnelId: string;
}

/** 配对成功响应：令牌只在此处下发一次。 */
export interface LanAccessPairResult {
  token: string;
  serverId: string;
  name?: string;
  /**
   * 被控端已配置公网中继时下发；主控据此自动写入公网端点，
   * 不需要用户再手填公网地址（docs/specs/lan-remote-public-endpoints.md）。
   */
  publicRelay?: LanPublicRelayInfo;
}

/** 客户端发起的配对请求（host 侧执行 HTTP，令牌落凭据服务）。 */
export interface LanPairPeerRequest {
  host: string;
  port: number;
  code: string;
  label?: string;
  /**
   * 主端点协议；缺省 http（LAN 直连）。https 用于经目录/内置中继的公网配对，
   * 必须能通过证书校验（docs/specs/lan-remote-public-endpoints.md）。
   */
  scheme?: "http" | "https";
  /** 主端点的路径前缀（中继/反代，如 `/d/<tunnelId>`）；缺省根路径。 */
  path?: string;
}

/** 凭据服务键：已配对客户端注册表（JSON 数组）。 */
export const LAN_ACCESS_CLIENTS_CREDENTIAL_KEY = "lan:access:clients";
/** 凭据服务键：安装级 serverId。 */
export const LAN_ACCESS_SERVER_ID_CREDENTIAL_KEY = "lan:access:server-id";

/** 单个客户端的长期令牌凭据键。 */
export function buildLanAccessClientTokenKey(clientId: string): string {
  return `lan:access:client:${clientId}:token`;
}

/** 客户端侧保存对端令牌的凭据键（host:port + serverId 稳定区分同一地址的不同实例）。 */
export function buildLanPeerTokenKey(serverId: string): string {
  return `lan:peer:${serverId}:token`;
}

/**
 * 客户端侧配对元数据凭据键：与令牌同生命周期。
 * 内容（lanPeerMetaSchema）让设置页在不接触令牌的前提下列出「我配对的对端」。
 */
export function buildLanPeerMetaKey(serverId: string): string {
  return `lan:peer:${serverId}:meta`;
}

/** 对端端点类型：LAN 直连（http）或公网 HTTPS。 */
export const lanPeerEndpointKindSchema = z.enum(["lan", "public"]);
export type LanPeerEndpointKind = z.infer<typeof lanPeerEndpointKindSchema>;

/**
 * 一个已配对对端的可达端点。公网端点只允许 https（docs/specs/lan-remote-public-endpoints.md
 * 产品规则 2）；schema 允许 http 是为了旧记录迁移与测试夹具，写入端必须校验。
 */
export const lanPeerEndpointSchema = z
  .object({
    scheme: z.enum(["http", "https"]),
    host: lanTrimmedString,
    port: z.number().int().positive().max(65535),
    kind: lanPeerEndpointKindSchema,
    /**
     * 反向代理/中继的路径前缀（如 `/d/<tunnelId>`）。缺省 = 根路径；
     * 远端 API/WS 都追加在该前缀之后（docs/specs/wss-relay-mvp.md）。
     */
    path: z
      .string()
      .trim()
      .max(512)
      .regex(/^\/[^?#]*$/u, "path must start with / and contain no query/hash")
      .optional(),
  })
  .strict();
export type LanPeerEndpoint = z.infer<typeof lanPeerEndpointSchema>;

/** v2：一个对端可有多个端点（公网优先、LAN 兜底）。 */
const lanPeerMetaEndpointsSchema = z
  .object({
    endpoints: z.array(lanPeerEndpointSchema).min(1).max(8),
    name: z.string().trim().optional(),
    pairedAt: z.number().int().nonnegative().optional(),
  })
  .strict();

/** v1：单地址；读取时迁移为 LAN http 端点，不要求用户重新配对。 */
const lanPeerMetaLegacySchema = z
  .object({
    host: lanTrimmedString,
    port: z.number().int().positive().max(65535),
    name: z.string().trim().optional(),
    pairedAt: z.number().int().nonnegative().optional(),
  })
  .strict();

/** 配对时固化的对端元数据：端点和展示名，用于设置页列表与后续重连展示。 */
export const lanPeerMetaSchema = z.union([lanPeerMetaEndpointsSchema, lanPeerMetaLegacySchema]);

/** 归一化后的元数据（v1 已迁移为 endpoints）。 */
export interface LanPeerMeta {
  endpoints: LanPeerEndpoint[];
  name?: string;
  pairedAt?: number;
}

/**
 * 端点排序：公网在前、LAN 在后（公网优先，连不上再回退局域网）；
 * 同类保持原顺序（Array#sort 稳定）。顺序即连接优先级。
 */
export function sortLanPeerEndpoints(endpoints: readonly LanPeerEndpoint[]): LanPeerEndpoint[] {
  return [...endpoints].sort((left, right) => {
    if (left.kind === right.kind) {
      return 0;
    }
    return left.kind === "public" ? -1 : 1;
  });
}

/** 按 scheme + host + port 去重，保留首次出现的顺序。 */
export function dedupeLanPeerEndpoints(endpoints: readonly LanPeerEndpoint[]): LanPeerEndpoint[] {
  const seen = new Set<string>();
  const result: LanPeerEndpoint[] = [];
  for (const endpoint of endpoints) {
    const key = `${endpoint.scheme}://${endpoint.host}:${endpoint.port}${endpoint.path ?? ""}`;
    if (seen.has(key)) {
      continue;
    }
    seen.add(key);
    result.push(endpoint);
  }
  return result;
}

/** 把凭据里的原始 JSON 归一化为 v2 元数据；无法识别时返回 null。 */
export function normalizeLanPeerMeta(value: unknown): LanPeerMeta | null {
  const parsed = lanPeerMetaSchema.safeParse(value);
  if (!parsed.success) {
    return null;
  }
  const data = parsed.data;
  if ("endpoints" in data) {
    // 公网端点只允许 HTTPS；即使凭据被手改成 http，也不让它进入连接路径。
    const endpoints = sortLanPeerEndpoints(
      dedupeLanPeerEndpoints(data.endpoints).filter(
        (endpoint) => endpoint.kind !== "public" || endpoint.scheme === "https",
      ),
    );
    if (endpoints.length === 0) {
      return null;
    }
    return {
      endpoints,
      ...(data.name ? { name: data.name } : {}),
      ...(data.pairedAt !== undefined ? { pairedAt: data.pairedAt } : {}),
    };
  }
  return {
    endpoints: [{ scheme: "http", host: data.host, port: data.port, kind: "lan" }],
    ...(data.name ? { name: data.name } : {}),
    ...(data.pairedAt !== undefined ? { pairedAt: data.pairedAt } : {}),
  };
}

export function parseLanPeerMeta(raw: string | null): LanPeerMeta | null {
  if (!raw) {
    return null;
  }
  try {
    return normalizeLanPeerMeta(JSON.parse(raw));
  } catch {
    return null;
  }
}

/**
 * 解析公网基础地址：只接受无 userinfo、无 path/query/hash 的 HTTPS 地址；
 * 允许省略协议（rd.example.com → https://rd.example.com）与端口（默认 443）。
 * 返回 null = 非法，调用方必须拒绝而不是猜测。
 */
export function parseLanPublicBaseUrl(value: string): LanPeerEndpoint | null {
  const trimmed = value.trim();
  if (!trimmed) {
    return null;
  }
  const hasScheme = /^[a-z][a-z0-9+.-]*:\/\//iu.test(trimmed);
  if (hasScheme && !/^https:\/\//iu.test(trimmed)) {
    return null;
  }
  let url: URL;
  try {
    url = new URL(hasScheme ? trimmed : `https://${trimmed}`);
  } catch {
    return null;
  }
  if (url.protocol !== "https:") {
    return null;
  }
  if (url.username || url.password) {
    return null;
  }
  if (url.search || url.hash) {
    return null;
  }
  // 允许中继/反代的路径前缀；根路径归一为缺省，尾部斜杠去掉。
  const path = url.pathname === "/" ? "" : url.pathname.replace(/\/+$/u, "");
  if (path.length > 512) {
    return null;
  }
  const port = url.port ? Number.parseInt(url.port, 10) : 443;
  if (!Number.isInteger(port) || port <= 0 || port > 65535) {
    return null;
  }
  if (!url.hostname) {
    return null;
  }
  return {
    scheme: "https",
    host: url.hostname,
    port,
    kind: "public",
    ...(path ? { path } : {}),
  };
}

/** 端点展示串：https:443 / http:80 省略端口。 */
export function formatLanPeerEndpoint(endpoint: LanPeerEndpoint): string {
  const defaultPort = endpoint.scheme === "https" ? 443 : 80;
  return `${endpoint.scheme}://${endpoint.host}${
    endpoint.port === defaultPort ? "" : `:${endpoint.port}`
  }${endpoint.path ?? ""}`;
}

/** 设置页「我配对的对端」一行；host 为空表示该配对缺少元数据（地址未知，仍可删除）。 */
export const lanPairedPeerSchema = z
  .object({
    serverId: lanTrimmedString,
    host: z.string(),
    port: z.number().int().nonnegative().max(65535),
    endpoints: z.array(lanPeerEndpointSchema).optional(),
    name: z.string().optional(),
    pairedAt: z.number().int().nonnegative().optional(),
  })
  .strict();
export type LanPairedPeer = z.infer<typeof lanPairedPeerSchema>;

/** 对端工作区目录快照（按需拉取；拉取失败由调用方转成「无法获取」）。 */
export const lanPairedPeerWorkspacesSchema = z
  .object({
    name: z.string().optional(),
    workspaces: z.array(serverRemoteWorkspaceInfoSchema),
  })
  .strict();
export type LanPairedPeerWorkspaces = z.infer<typeof lanPairedPeerWorkspacesSchema>;

/** 6 位大写十六进制配对码格式校验（生成侧在 Host，避免把 node:crypto 带进 Web 包）。 */
export function isLanAccessPairCodeFormat(code: string): boolean {
  return /^[0-9A-F]{6}$/u.test(code.trim().toUpperCase());
}

/**
 * 对端工作区暴露过滤（spec: docs/specs/lan-paired-devices.md）：
 * 只把「用户选择的路径」暴露给局域网对端；本机默认对话工作区（数据根下的
 * workspace/default，非项目会话目录）是内部目录，不外发。
 * 纯函数：大小写比较策略由调用方传入（win32 不敏感、POSIX 精确），收尾分隔符先归一。
 */
export function isInternalConversationWorkspacePath(
  workspacePath: string,
  conversationWorkspaceDir: string,
  options: { caseInsensitive?: boolean } = {},
): boolean {
  const normalize = (value: string): string => value.replace(/[\\/]+$/u, "");
  const left = normalize(workspacePath);
  const right = normalize(conversationWorkspaceDir);
  return options.caseInsensitive ? left.toLowerCase() === right.toLowerCase() : left === right;
}
