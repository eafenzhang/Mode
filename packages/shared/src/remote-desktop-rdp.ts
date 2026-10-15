/**
 * 远程桌面（系统 RDP）主控端形状真源（docs/specs/remote-desktop-system-rdp.md 接口节）。
 *
 * 沿用 `remote-desktop` 的独立入口约定：根 barrel 不重导出，避免把桌面链路类型带进
 * 所有消费方（`@mode/shared/remote-desktop-rdp`）。
 *
 * 注意：本方案下应用**不参与会话内容**——画面/输入/剪贴板/音频全部由 Windows RDP 承担，
 * 这里只有「连接选项」与「会话进程记录」两类形状。
 */
import { z } from "zod";

/** 系统 RDP 默认端口（**不是** Mode LAN server 端口）。 */
export const DEFAULT_RDP_PORT = 3389;

/** 音频重定向目标（`audiomode`）：带到本机 / 留在被控 / 禁用。 */
export const rdpAudioModeSchema = z.enum(["local", "remote", "disabled"]);
export type RdpAudioMode = z.infer<typeof rdpAudioModeSchema>;

/** 每设备连接选项（主控端偏好，按 serverId 记忆）。 */
export const rdpConnectionOptionsSchema = z
  .object({
    /** 全屏（`screen mode id:i:2`）；缺省窗口。 */
    fullscreen: z.boolean().optional(),
    /** 多显示器同时铺开（`use multimon:i:1`）；缺省关。 */
    multimon: z.boolean().optional(),
    /** 剪贴板重定向；缺省开。 */
    clipboard: z.boolean().optional(),
    /** 音频重定向目标；缺省 local。 */
    audio: rdpAudioModeSchema.optional(),
    /** 预填用户名（不含密码）。 */
    username: z.string().optional(),
    /** RDP 端口；缺省 3389。 */
    port: z.number().int().min(1).max(65535).optional(),
  })
  .strict();
export type RdpConnectionOptions = z.infer<typeof rdpConnectionOptionsSchema>;

/** 应用内可见的一条 RDP 会话（仅本进程启动的；不跨重启持久化）。 */
export const rdpSessionRecordSchema = z
  .object({
    sessionId: z.string().trim().min(1),
    serverId: z.string().trim().min(1),
    name: z.string(),
    address: z.string().trim().min(1),
    pid: z.number().int().positive(),
    startedAt: z.number().int().nonnegative(),
  })
  .strict();
export type RdpSessionRecord = z.infer<typeof rdpSessionRecordSchema>;

/**
 * 连接失败原因（稳定值，UI 据此出文案）。
 * `invalid_address` / `invalid_username` = `.rdp` 注入校验拦下（见 launcher 头注释）；
 * `launch_failed` = 临时文件写入或 mstsc 启动失败。
 */
export const rdpConnectFailureReasonSchema = z.enum([
  "invalid_address",
  "invalid_username",
  "launch_failed",
]);
export type RdpConnectFailureReason = z.infer<typeof rdpConnectFailureReasonSchema>;

export const rdpConnectResultSchema = z.discriminatedUnion("ok", [
  z.object({ ok: z.literal(true), session: rdpSessionRecordSchema }).strict(),
  z.object({ ok: z.literal(false), reason: rdpConnectFailureReasonSchema }).strict(),
]);
export type RdpConnectResult = z.infer<typeof rdpConnectResultSchema>;

/** 连接请求（renderer → main，单参数结构化载荷，便于 main 侧 `.strict()` 校验）。 */
export const rdpConnectRequestSchema = z
  .object({
    serverId: z.string().trim().min(1),
    /** 设备显示名（日志与失败文案用；不参与 .rdp 内容）。 */
    name: z.string(),
    /** RDP 地址（对端 LAN host/IP）。由 launcher 整串校验，防 `.rdp` 注入。 */
    address: z.string().trim().min(1),
    options: rdpConnectionOptionsSchema,
  })
  .strict();
export type RdpConnectRequest = z.infer<typeof rdpConnectRequestSchema>;

/** 被控端设置面状态（共享开关 + 系统 RDP 真实状态）。 */
export const rdpServerStateSchema = z
  .object({
    /** 提示性共享开关（**不是**安全闸门，见 spec 产品规则 5）。 */
    sharingEnabled: z.boolean(),
    /** 系统 RDP 状态；探测未就绪/失败 = 缺键（UI 按状态未知呈现）。 */
    systemRdp: z.object({ enabled: z.boolean(), editionOk: z.boolean() }).strict().optional(),
  })
  .strict();
export type RdpServerState = z.infer<typeof rdpServerStateSchema>;
