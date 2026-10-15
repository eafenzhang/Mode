import { z } from "zod";

export const SERVER_REMOTE_PROTOCOL_VERSION = 1;

export const serverRemoteWorkspaceInfoSchema = z.object({
  path: z.string().trim().min(1),
  label: z.string().trim().min(1).optional(),
  workspaceIdentity: z.string().trim().min(1).optional(),
});

/** 系统 RDP 状态能力位（被控端报告；主控设备卡据此置灰与给指引）。 */
export const systemRdpCapabilitySchema = z
  .object({ enabled: z.boolean(), editionOk: z.boolean() })
  .strict();

export type SystemRdpCapability = z.infer<typeof systemRdpCapabilitySchema>;

export const serverRemoteInfoSchema = z.object({
  serverId: z.string().trim().min(1),
  name: z.string().trim().min(1).optional(),
  version: z.string(),
  protocolVersion: z.literal(SERVER_REMOTE_PROTOCOL_VERSION),
  authRequired: z.boolean(),
  workspaces: z.array(serverRemoteWorkspaceInfoSchema),
  capabilities: z.object({
    desktopContinuous: z.literal(true),
    websocketRpc: z.literal(true),
    // 旧 Server 缺少新增 dynamic event，必须先声明能力再订阅，避免异常打进对端读循环。
    // LAN 电脑控制设备能力位：对端 win/mac + env kill-switch + desktop-local host 才发 true；
    // 旧对端缺键 = 不支持（docs/specs/lan-remote-computer-use.md）。
    computerUse: z.boolean().optional(),
    // LAN 远程桌面提示性共享位：对端 win32 + 共享开关开 + env kill-switch 才发 true。
    // **不是安全闸门**——连接由 Windows RDP 承接，本应用拦不住；它只决定本机是否作为
    // 可远控设备出现在对端设备页（docs/specs/remote-desktop-system-rdp.md 产品规则 5）。
    remoteDesktop: z.boolean().optional(),
    // 系统 RDP 真实状态（同 spec 接口节）：enabled 取注册表 fDenyTSConnections、
    // editionOk 取 Windows 版本是否支持承载 RDP 主机（家庭版为 false）。
    // 握手即时计算、无持久化；旧对端缺键 = 状态未知（设备卡不置灰，失败由 mstsc 自身给出）。
    systemRdp: systemRdpCapabilitySchema.optional(),
  }),
});

export type ServerRemoteWorkspaceInfo = z.infer<typeof serverRemoteWorkspaceInfoSchema>;

export type ServerRemoteInfo = z.infer<typeof serverRemoteInfoSchema>;

export const serverRemoteHostCapabilitySchema = z
  .object({
    capability: z.string().trim().min(1),
    expiresAt: z.number().int().positive(),
  })
  .strict();

export type ServerRemoteHostCapability = z.infer<typeof serverRemoteHostCapabilitySchema>;
