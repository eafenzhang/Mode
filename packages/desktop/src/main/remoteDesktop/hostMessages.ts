/**
 * 远程桌面 main ↔ host 桥的消息面（wire 契约）。
 *
 * 重要：本文件与 `packages/desktop/src/host/remoteDesktopMessages.ts` 互为**逐字镜像**，
 * 任何一侧改动必须双端同步；漂移由 `packages/desktop/tests/remote-desktop-host-messages.test.mts`
 * 逐条比对类型表与交叉解析结果来兜住。
 *
 * 为什么不进 `@mode/shared` 的 `HostMessageTypes` / `hostIncomingMessageSchema` /
 * `hostResponseMessageSchema`：那三处是本次任务的冻结面（只允许改 desktop 的 main/host 新代码），
 * 而 main 与 host 是两个独立 tsconfig `rootDir`，无法共享同一个模块文件。因此 RD 消息族改为
 * 「桌面包本地 zod schema」：host 侧在共享 `hostIncomingMessageSchema` 校验**之前**按类型标签分流
 * （见 `host/index.ts` 的 parentPort 监听入口），main 侧在 `hostResponseMessageSchema` 校验之前同样分流。
 * 校验强度与共享消息一致（zod `.strict()`、未知键/越界一律拒绝），只是不再走共享联合体。
 *
 * 方向与事件顺序（docs/specs/lan-remote-desktop.md 事件顺序 ②③⑤⑥）：
 *
 *   host → main：ConsentRequest②、SignalFromHost③、SessionClosed⑥、
 *                ServerStateResult(设置面)、PeerProbeResult / PeerSessionOpenResult(主控⑤)、
 *                MediaStatus(P2 原生媒体面冻结通知)
 *   main → host：ConsentResult②、SignalToHost③、Inject⑤、
 *                ServerStateRequest / SetServerEnabled / RevokeClient(设置面)、
 *                PeerProbe / PeerSessionOpen(主控侧读对端凭据与发起请求)、
 *                MediaStart / MediaSwitch / MediaStop(P2 原生媒体面开流/换屏/停流)
 *
 * 端口/帧上限：信令单帧 ≤64KiB（超限 host 断开），注入每帧 ≤512 条（超限丢弃并计数，与
 * Rust `MAX_RAW_INPUT_EVENTS` 同口径）。
 */
import { z } from "zod";
import { remoteDesktopSignalMessageSchema } from "@mode/shared/remote-desktop";

/** 一条注入帧的最大事件条数（与 crates/mode-cua-ax input.rs 的 MAX_RAW_INPUT_EVENTS 同口径）。 */
export const REMOTE_DESKTOP_INJECT_MAX_EVENTS = 512;
/** 信令单帧上限（utf8 字节）：超限 host 断开该会话，不做截断转发。 */
export const REMOTE_DESKTOP_SIGNAL_MAX_FRAME_BYTES = 64 * 1024;

const trimmedNonEmpty = z.string().trim().min(1);

/**
 * 消息类型表（冻结命名）。键名沿用交付约定的 `RemoteDesktop*`，值即 postMessage 的 type。
 */
export const RemoteDesktopHostMessageTypes = {
  /** host → main：被控 main 弹同意（已记住的设备由 main 决策自动放行）。 */
  RemoteDesktopConsentRequest: "remote-desktop-consent-request",
  /** main → host：同意结果（remember=true 时 host 把授权写回 lan client 记录）。 */
  RemoteDesktopConsentResult: "remote-desktop-consent-result",
  /** host → main：WS 收到的信令帧（host 不解析内容）。 */
  RemoteDesktopSignalFromHost: "remote-desktop-signal-from-host",
  /** main → host：要写回 WS 的信令帧。 */
  RemoteDesktopSignalToHost: "remote-desktop-signal-to-host",
  /** host → main：会话结束（对端 bye / WS 关闭 / 错误），main 据此收口。 */
  RemoteDesktopSessionClosed: "remote-desktop-session-closed",
  /** main → host：注入帧（坐标已换算为 VIRTUALDESK 0..65535）。 */
  RemoteDesktopInject: "remote-desktop-inject",
  /** main → host：读被控设置（总开关 + 已授权设备）。 */
  RemoteDesktopServerStateRequest: "remote-desktop-server-state-request",
  /** host → main：被控设置回帖（按 requestId 关联）。 */
  RemoteDesktopServerStateResult: "remote-desktop-server-state-result",
  /** main → host：设置被控总开关（唯一写路径在 host 的凭据存储）。 */
  RemoteDesktopSetServerEnabled: "remote-desktop-server-enabled-set",
  /** main → host：撤销某设备的远程桌面授权。 */
  RemoteDesktopRevokeClient: "remote-desktop-client-revoke",
  /** main → host：用已存令牌探对端 /api/server-info 能力位（令牌不出 host）。 */
  RemoteDesktopPeerProbe: "remote-desktop-peer-probe",
  /** host → main：对端探测结果（失败 → online:false）。 */
  RemoteDesktopPeerProbeResult: "remote-desktop-peer-probe-result",
  /** main → host：用已存令牌请求对端受控会话（POST /api/remote-desktop/session）。 */
  RemoteDesktopPeerSessionOpen: "remote-desktop-peer-session-open",
  /** host → main：对端会话请求结果（ok 时带一次性票据与 WS 地址）。 */
  RemoteDesktopPeerSessionOpenResult: "remote-desktop-peer-session-open-result",
  /** main → host：开媒体采集（同意后下发；key/salt 同帧带给 helper，通知型无 requestId）。 */
  RemoteDesktopMediaStart: "remote-desktop-media-start",
  /** main → host：换屏（select-monitor 解析出物理 rect 后下发）。 */
  RemoteDesktopMediaSwitch: "remote-desktop-media-switch",
  /** main → host：停媒体采集（stop-capture / 会话收口）。 */
  RemoteDesktopMediaStop: "remote-desktop-media-stop",
  /** host → main：媒体采集状态（helper kind1 透传；冻结透传给主控渲染占位）。 */
  RemoteDesktopMediaStatus: "remote-desktop-media-status",
} as const;

/** 会话关闭原因（host → main）：对端主动 bye / WS 断开 / 内部错误。 */
export const remoteDesktopSessionClosedReasonSchema = z.enum(["peer-end", "socket-closed", "error"]);
export type RemoteDesktopSessionClosedReason = z.infer<typeof remoteDesktopSessionClosedReasonSchema>;

/** 注入事件（与 crates/mode-cua-ax `parse_raw_event` 的严格口径一致：未知键即拒绝）。 */
export const remoteDesktopRawInputEventSchema = z.discriminatedUnion("k", [
  z.object({ k: z.literal("move"), x: z.number().int().min(0).max(65535), y: z.number().int().min(0).max(65535) }).strict(),
  z
    .object({
      k: z.literal("button"),
      button: z.enum(["left", "right", "middle"]),
      down: z.boolean(),
      x: z.number().int().min(0).max(65535),
      y: z.number().int().min(0).max(65535),
    })
    .strict(),
  z
    .object({
      k: z.literal("wheel"),
      direction: z.enum(["up", "down", "left", "right"]),
      amount: z.number().finite().min(0).max(20),
      x: z.number().int().min(0).max(65535),
      y: z.number().int().min(0).max(65535),
    })
    .strict(),
  z.object({ k: z.literal("key"), vk: z.number().int().min(1).max(255), down: z.boolean() }).strict(),
  z.object({ k: z.literal("text"), text: z.string().min(1).max(4096) }).strict(),
]);
export type RemoteDesktopRawInputEvent = z.infer<typeof remoteDesktopRawInputEventSchema>;

// ---- host → main ----

export const remoteDesktopConsentRequestSchema = z
  .object({
    type: z.literal(RemoteDesktopHostMessageTypes.RemoteDesktopConsentRequest),
    clientId: trimmedNonEmpty,
    label: z.string(),
  })
  .strict();

export const remoteDesktopSignalFromHostSchema = z
  .object({
    type: z.literal(RemoteDesktopHostMessageTypes.RemoteDesktopSignalFromHost),
    message: remoteDesktopSignalMessageSchema,
  })
  .strict();

export const remoteDesktopSessionClosedSchema = z
  .object({
    type: z.literal(RemoteDesktopHostMessageTypes.RemoteDesktopSessionClosed),
    reason: remoteDesktopSessionClosedReasonSchema,
  })
  .strict();

export const remoteDesktopAuthorizedClientSchema = z
  .object({ id: trimmedNonEmpty, label: z.string() })
  .strict();

export const remoteDesktopServerStateResultSchema = z
  .object({
    type: z.literal(RemoteDesktopHostMessageTypes.RemoteDesktopServerStateResult),
    requestId: trimmedNonEmpty,
    ok: z.boolean(),
    enabled: z.boolean(),
    authorizedClients: z.array(remoteDesktopAuthorizedClientSchema),
    error: z.string().optional(),
  })
  .strict();

export const remoteDesktopPeerProbeResultSchema = z
  .object({
    type: z.literal(RemoteDesktopHostMessageTypes.RemoteDesktopPeerProbeResult),
    requestId: trimmedNonEmpty,
    online: z.boolean(),
    remoteDesktop: z.boolean(),
    name: z.string().optional(),
  })
  .strict();

export const remoteDesktopPeerSessionOpenResultSchema = z
  .object({
    type: z.literal(RemoteDesktopHostMessageTypes.RemoteDesktopPeerSessionOpenResult),
    requestId: trimmedNonEmpty,
    ok: z.boolean(),
    wsUrl: z.string().optional(),
    // 契约 E：媒体面连接地址（host 从 POST 响应拼出）。strict schema 漏字段会整条丢弃，
    // 必须与 host 侧上报同步声明，否则主控 start 挂到超时。
    mediaUrl: z.string().optional(),
    errorCode: z.string().optional(),
    error: z.string().optional(),
  })
  .strict();

export const remoteDesktopMediaStatusSchema = z
  .object({
    type: z.literal(RemoteDesktopHostMessageTypes.RemoteDesktopMediaStatus),
    frozen: z.boolean(),
    // helper 侧实现面错误串（capture-error / encode-error / access-lost / locked…），
    // 原样透传；协议面归一在 coordinator（freeze{reason} union 只认 locked|access-lost）。
    reason: z.string().optional(),
  })
  .strict();

// ---- main → host ----

export const remoteDesktopConsentResultSchema = z
  .object({
    type: z.literal(RemoteDesktopHostMessageTypes.RemoteDesktopConsentResult),
    clientId: trimmedNonEmpty,
    decision: z.enum(["allow", "deny"]),
    remember: z.boolean(),
    // 超时与用户拒绝同为 deny，但对端需要的稳定错误码不同（consent_timeout / consent_denied）。
    reason: z.enum(["user", "timeout"]).optional(),
  })
  .strict();

export const remoteDesktopSignalToHostSchema = z
  .object({
    type: z.literal(RemoteDesktopHostMessageTypes.RemoteDesktopSignalToHost),
    message: remoteDesktopSignalMessageSchema,
  })
  .strict();

export const remoteDesktopInjectSchema = z
  .object({
    type: z.literal(RemoteDesktopHostMessageTypes.RemoteDesktopInject),
    events: z.array(remoteDesktopRawInputEventSchema).max(REMOTE_DESKTOP_INJECT_MAX_EVENTS),
  })
  .strict();

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

export const remoteDesktopRevokeClientSchema = z
  .object({
    type: z.literal(RemoteDesktopHostMessageTypes.RemoteDesktopRevokeClient),
    requestId: trimmedNonEmpty,
    clientId: trimmedNonEmpty,
  })
  .strict();

export const remoteDesktopPeerProbeSchema = z
  .object({
    type: z.literal(RemoteDesktopHostMessageTypes.RemoteDesktopPeerProbe),
    requestId: trimmedNonEmpty,
    serverId: trimmedNonEmpty,
  })
  .strict();

export const remoteDesktopPeerSessionOpenSchema = z
  .object({
    type: z.literal(RemoteDesktopHostMessageTypes.RemoteDesktopPeerSessionOpen),
    requestId: trimmedNonEmpty,
    serverId: trimmedNonEmpty,
  })
  .strict();

/** 媒体面矩形（物理像素域，与 helper rd_media_start/switch 的 rect 口径一致；x/y 允许多屏负原点）。 */
export const remoteDesktopMediaRectSchema = z
  .object({
    x: z.number().int(),
    y: z.number().int(),
    width: z.number().int().positive(),
    height: z.number().int().positive(),
  })
  .strict();
export type RemoteDesktopMediaRect = z.infer<typeof remoteDesktopMediaRectSchema>;

export const remoteDesktopMediaStartSchema = z
  .object({
    type: z.literal(RemoteDesktopHostMessageTypes.RemoteDesktopMediaStart),
    rect: remoteDesktopMediaRectSchema,
    // 与 media-open 信令同口径：64 位小写 hex（32B key）/ 16 位小写 hex（8B salt）。
    key: z.string().regex(/^[0-9a-f]{64}$/u),
    salt: z.string().regex(/^[0-9a-f]{16}$/u),
  })
  .strict();

export const remoteDesktopMediaSwitchSchema = z
  .object({
    type: z.literal(RemoteDesktopHostMessageTypes.RemoteDesktopMediaSwitch),
    rect: remoteDesktopMediaRectSchema,
  })
  .strict();

export const remoteDesktopMediaStopSchema = z
  .object({
    type: z.literal(RemoteDesktopHostMessageTypes.RemoteDesktopMediaStop),
  })
  .strict();

/** host 侧入口：全部 main → host 消息（未知 type 一律拒绝，不进入共享 schema）。 */
export const remoteDesktopMessageToHostSchema = z.discriminatedUnion("type", [
  remoteDesktopConsentResultSchema,
  remoteDesktopSignalToHostSchema,
  remoteDesktopInjectSchema,
  remoteDesktopServerStateRequestSchema,
  remoteDesktopSetServerEnabledSchema,
  remoteDesktopRevokeClientSchema,
  remoteDesktopPeerProbeSchema,
  remoteDesktopPeerSessionOpenSchema,
  remoteDesktopMediaStartSchema,
  remoteDesktopMediaSwitchSchema,
  remoteDesktopMediaStopSchema,
]);
export type RemoteDesktopMessageToHost = z.infer<typeof remoteDesktopMessageToHostSchema>;

/** main 侧入口：全部 host → main 消息。 */
export const remoteDesktopMessageFromHostSchema = z.discriminatedUnion("type", [
  remoteDesktopConsentRequestSchema,
  remoteDesktopSignalFromHostSchema,
  remoteDesktopSessionClosedSchema,
  remoteDesktopServerStateResultSchema,
  remoteDesktopPeerProbeResultSchema,
  remoteDesktopPeerSessionOpenResultSchema,
  remoteDesktopMediaStatusSchema,
]);
export type RemoteDesktopMessageFromHost = z.infer<typeof remoteDesktopMessageFromHostSchema>;

/**
 * 类型标签全集（漂移测试与快速分流共用）：main/host 双端都必须逐字一致。
 * 用 `Array.isArray` 之外的最廉价判断给 parentPort 入口分流，避免对每条共享消息都跑 RD schema。
 */
export const REMOTE_DESKTOP_MESSAGE_TYPE_TAGS: readonly string[] = Object.freeze(
  Object.values(RemoteDesktopHostMessageTypes),
);

export function isRemoteDesktopMessageTypeTag(value: unknown): boolean {
  return typeof value === "string" && REMOTE_DESKTOP_MESSAGE_TYPE_TAGS.includes(value);
}

// 主控方向的对端错误码用 RemoteDesktopErrorCode 的字符串面（避免在此模块再引入一次类型别名）。
