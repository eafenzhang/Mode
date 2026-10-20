import { z } from "zod";

/**
 * Mode Agent 反向配对（docs/specs/legacy-windows-remote-agent.md）：
 * 桌面端生成一次性配对码，被控端安装弹窗输入后 POST 到桌面端 LAN 监听；
 * 桌面端消费配对码并把被控端写成用户级 HTTP MCP server。
 */
export const REMOTE_AGENT_ENROLLMENT_PATH = "/api/remote-agent/enroll";

export const remoteAgentEnrollmentRequestSchema = z
  .object({
    code: z.string().trim().min(4).max(32),
    agentId: z.string().trim().min(1).max(128),
    name: z.string().trim().max(128).optional(),
    host: z.string().trim().min(1).max(255),
    port: z.number().int().positive().max(65535),
    token: z.string().trim().min(16).max(512),
    version: z.string().trim().max(64).optional(),
    platform: z.string().trim().max(32).optional(),
    addresses: z.array(z.string().trim().min(1).max(255)).max(16).optional(),
  })
  .strict();

export type RemoteAgentEnrollmentRequest = z.infer<typeof remoteAgentEnrollmentRequestSchema>;

export interface RemoteAgentEnrollmentResult {
  ok: boolean;
  error?: string;
  serverName?: string;
}

export interface RemoteAgentEnrollmentEndpoint {
  enroll(request: RemoteAgentEnrollmentRequest): Promise<RemoteAgentEnrollmentResult>;
}
