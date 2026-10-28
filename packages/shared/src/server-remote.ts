import { z } from "zod";

export const SERVER_REMOTE_PROTOCOL_VERSION = 1;

export const serverRemoteWorkspaceInfoSchema = z.object({
  path: z.string().trim().min(1),
  label: z.string().trim().min(1).optional(),
  workspaceIdentity: z.string().trim().min(1).optional(),
});

export const serverRemoteInfoSchema = z.object({
  serverId: z.string().trim().min(1),
  name: z.string().trim().min(1).optional(),
  version: z.string(),
  protocolVersion: z.literal(SERVER_REMOTE_PROTOCOL_VERSION),
  authRequired: z.boolean(),
  workspaces: z.array(serverRemoteWorkspaceInfoSchema),
  /**
   * 内置公网中继：被控端已配置中继时随 server-info 下发，主控可据此自动补全
   * 公网端点；旧对端缺键 = 未配置（docs/specs/lan-remote-public-endpoints.md）。
   */
  publicRelay: z
    .object({
      baseUrl: z.string().url(),
      tunnelId: z.string().trim().min(16).max(128),
    })
    .strict()
    .optional(),
  capabilities: z.object({
    desktopContinuous: z.literal(true),
    websocketRpc: z.literal(true),
    // 旧 Server 缺少新增 dynamic event，必须先声明能力再订阅，避免异常打进对端读循环。
    // LAN 电脑控制设备能力位：对端 win/mac + env kill-switch + desktop-local host 才发 true；
    // 旧对端缺键 = 不支持（docs/specs/lan-remote-computer-use.md）。
    computerUse: z.boolean().optional(),
    // LAN 远程桌面设备能力位：对端 win32 + 被控开关开 + env kill-switch 才发 true；
    // 旧对端缺键 = 不支持（docs/specs/lan-remote-desktop.md）。
    remoteDesktop: z.boolean().optional(),
    // 原生媒体面版本门（纯直替兼容闸）：本期原生媒体面就是唯一媒体面，
    // 旧对端缺键 = 不支持（docs/specs/lan-remote-desktop-native-media.md 版本门 1）。
    remoteDesktopNative: z.boolean().optional(),
    // Web/fpk 作为 LAN 主控：服务端提供 /api/lan/peers 与 /api/connect-remote-lan。
    // 桌面 LAN 静态站/旧服务端缺键 = 没有主控面，远程控制页不展示
    //（docs/specs/web-remote-workspace.md）。
    lanPeerController: z.boolean().optional(),
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
