import { z } from "zod";

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
}

export function buildLanProbePayload(): string {
  return LAN_ACCESS_PROBE;
}

export function buildLanAnnouncementPayload(
  info: Omit<LanPeerAnnouncement, "magic" | "protocolVersion">,
): string {
  return JSON.stringify({ magic: "mode-lan", protocolVersion: LAN_ACCESS_PROTOCOL_VERSION, ...info });
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

/** 配对成功响应：令牌只在此处下发一次。 */
export interface LanAccessPairResult {
  token: string;
  serverId: string;
  name?: string;
}

/** 客户端发起的配对请求（host 侧执行 HTTP，令牌落凭据服务）。 */
export interface LanPairPeerRequest {
  host: string;
  port: number;
  code: string;
  label?: string;
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

/** 6 位大写十六进制配对码格式校验（生成侧在 Host，避免把 node:crypto 带进 Web 包）。 */
export function isLanAccessPairCodeFormat(code: string): boolean {
  return /^[0-9A-F]{6}$/u.test(code.trim().toUpperCase());
}
