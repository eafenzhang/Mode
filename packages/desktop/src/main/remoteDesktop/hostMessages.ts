/**
 * main ↔ host 的远程桌面消息契约（本文件在 host 与 main 各存一份，**必须逐字一致**；
 * 漂移由 `tests/remote-desktop-host-messages.test.mts`（夹具 `tests/fixtures/remote-desktop-host-messages.ts`）比对）。
 *
 * 系统 RDP 方案下应用不再承载会话（画面/输入/剪贴板/音频全由 Windows RDP 承担），
 * 故消息族只剩两类：
 * - 被控设置读写（提示性共享开关 + 系统 RDP 状态）；
 * - 主控方向的对端能力探测。
 * 同意窗、信令转发、媒体采集、输入注入、会话开启的消息族随自研媒体面一并删除
 * （docs/specs/remote-desktop-system-rdp.md）。
 */
import { z } from "zod";

const trimmedNonEmpty = z.string().trim().min(1);

/**
 * 消息类型表（命名冻结）。main 与 host 是不同进程入口，靠这份镜像对齐；
 * 改一处不改另一处即漂移（漂移夹具会红）。
 */
export const RemoteDesktopHostMessageTypes = {
  /** main → host：读被控设置（共享开关 + 系统 RDP 状态）。 */
  RemoteDesktopServerStateRequest: "remote-desktop-server-state-request",
  /** host → main：被控设置回帖（按 requestId 关联；ok=true 时 sharingEnabled 必有）。 */
  RemoteDesktopServerStateResult: "remote-desktop-server-state-result",
  /** main → host：写共享开关（唯一写路径在 host 的凭据存储）。 */
  RemoteDesktopSetServerEnabled: "remote-desktop-server-enabled-set",
  /** main → host：用已存令牌探对端 /api/server-info 能力位（令牌不出 host）。 */
  RemoteDesktopPeerProbe: "remote-desktop-peer-probe",
  /** host → main：对端探测结果（任何失败 → online:false）。 */
  RemoteDesktopPeerProbeResult: "remote-desktop-peer-probe-result",
} as const;

/**
 * 系统 RDP 状态（与 `@mode/shared` 的 `SystemRdpCapability` 同形）。
 * 此处独立声明而非跨入口 import shared：契约镜像要求两文件逐字一致，引外部类型会让
 * 漂移比对失去意义。改字段必须同时改 shared 与两份镜像（spec 接口节）。
 */
export const remoteDesktopSystemRdpSchema = z
  .object({ enabled: z.boolean(), editionOk: z.boolean() })
  .strict();

// ---- host → main ----

export const remoteDesktopServerStateResultSchema = z
  .object({
    type: z.literal(RemoteDesktopHostMessageTypes.RemoteDesktopServerStateResult),
    requestId: trimmedNonEmpty,
    ok: z.boolean(),
    /** 提示性共享开关；ok=true 时必带（**不是**安全闸门，见 spec 产品规则 5）。 */
    sharingEnabled: z.boolean().optional(),
    /** 系统 RDP 状态；缺省 = 探测未就绪或失败（状态未知，UI 不得据此置灰）。 */
    systemRdp: remoteDesktopSystemRdpSchema.optional(),
    error: z.string().optional(),
  })
  .strict();

export const remoteDesktopPeerProbeResultSchema = z
  .object({
    type: z.literal(RemoteDesktopHostMessageTypes.RemoteDesktopPeerProbeResult),
    requestId: trimmedNonEmpty,
    online: z.boolean(),
    /** 对端 `capabilities.remoteDesktop`（提示性共享位）；缺键 = false。 */
    remoteDesktop: z.boolean(),
    /** 对端系统 RDP 状态；缺省 = 旧对端或对端探测未就绪。 */
    systemRdp: remoteDesktopSystemRdpSchema.optional(),
    name: z.string().optional(),
  })
  .strict();

// ---- main → host ----

export const remoteDesktopServerStateRequestSchema = z
  .object({
    type: z.literal(RemoteDesktopHostMessageTypes.RemoteDesktopServerStateRequest),
    requestId: trimmedNonEmpty,
  })
  .strict();

export const remoteDesktopSetServerEnabledSchema = z
  .object({
    type: z.literal(RemoteDesktopHostMessageTypes.RemoteDesktopSetServerEnabled),
    requestId: trimmedNonEmpty,
    enabled: z.boolean(),
  })
  .strict();

export const remoteDesktopPeerProbeSchema = z
  .object({
    type: z.literal(RemoteDesktopHostMessageTypes.RemoteDesktopPeerProbe),
    requestId: trimmedNonEmpty,
    serverId: trimmedNonEmpty,
  })
  .strict();

/** host 侧入口：全部 main → host 消息（未知 type 一律拒绝，不进入共享 schema）。 */
export const remoteDesktopMessageToHostSchema = z.discriminatedUnion("type", [
  remoteDesktopServerStateRequestSchema,
  remoteDesktopSetServerEnabledSchema,
  remoteDesktopPeerProbeSchema,
]);
export type RemoteDesktopMessageToHost = z.infer<typeof remoteDesktopMessageToHostSchema>;

/** main 侧入口：全部 host → main 消息。 */
export const remoteDesktopMessageFromHostSchema = z.discriminatedUnion("type", [
  remoteDesktopServerStateResultSchema,
  remoteDesktopPeerProbeResultSchema,
]);
export type RemoteDesktopMessageFromHost = z.infer<typeof remoteDesktopMessageFromHostSchema>;

/**
 * 类型标签全集（漂移测试与快速分流共用）：main/host 双端都必须逐字一致。
 * 用最廉价的字符串命中给 parentPort 入口分流，避免对每条共享消息都跑 RD schema。
 */
export const REMOTE_DESKTOP_MESSAGE_TYPE_TAGS: readonly string[] = Object.freeze(
  Object.values(RemoteDesktopHostMessageTypes),
);

export function isRemoteDesktopMessageTypeTag(value: unknown): boolean {
  return typeof value === "string" && REMOTE_DESKTOP_MESSAGE_TYPE_TAGS.includes(value);
}

/**
 * host 入口分流：类型标签命中 RD 集合才解析（返回 null = 交给共享 schema）。
 * 标签命中但 schema 不合法也返回 null——调用方按「未消费」处理，避免双报错噪音。
 */
export function parseRemoteDesktopMessageToHost(data: unknown): RemoteDesktopMessageToHost | null {
  const type = (data as { type?: unknown } | null)?.type;
  if (!isRemoteDesktopMessageTypeTag(type)) {
    return null;
  }
  const parsed = remoteDesktopMessageToHostSchema.safeParse(data);
  return parsed.success ? parsed.data : null;
}
