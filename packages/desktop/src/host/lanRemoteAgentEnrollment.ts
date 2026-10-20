import { randomBytes } from "node:crypto";

import type {
  RemoteAgentEnrollmentRequest,
  RemoteAgentEnrollmentResult,
} from "@mode/shared";
import { IMcpSyncService, type ServiceCollection } from "@mode/services";
import { formatLogPrefix } from "@mode/shared";

const log = (...args: unknown[]) => console.log(formatLogPrefix("lanAccess", process.pid), ...args);

export interface CreateRemoteAgentEnrollerParams {
  services: ServiceCollection;
  /** 一次性消费桌面配对码；与 LAN 客户端配对共用，失败也作废。 */
  consumePairCode: (code: string) => boolean;
}

/**
 * 反向配对：被控端拿桌面生成的配对码登记自己。
 * 先探测 agent 的 /health，避免把不可达的地址写成 MCP；配对码与 LAN 配对共用，
 * 一次性消费，失败按分钟限速，防止 6 位码被暴力枚举。
 */
export function createRemoteAgentEnroller(
  params: CreateRemoteAgentEnrollerParams,
): (request: RemoteAgentEnrollmentRequest) => Promise<RemoteAgentEnrollmentResult> {
  let enrollmentFailures = 0;
  let enrollmentFailureResetAt = 0;

  return async (request) => {
    const now = Date.now();
    if (now > enrollmentFailureResetAt) {
      enrollmentFailures = 0;
      enrollmentFailureResetAt = now + 60_000;
    }
    if (enrollmentFailures >= 20) {
      return { ok: false, error: "too_many_attempts" };
    }
    if (!params.consumePairCode(request.code)) {
      enrollmentFailures += 1;
      return { ok: false, error: "invalid_pair_code" };
    }
    enrollmentFailures = 0;
    const mcpSync = params.services.getOptional(IMcpSyncService);
    if (!mcpSync) {
      return { ok: false, error: "mcp_unavailable" };
    }
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 3_000);
    try {
      const health = await fetch(`http://${request.host}:${request.port}/health`, {
        signal: controller.signal,
      });
      if (!health.ok) {
        return { ok: false, error: "agent_unreachable" };
      }
    } catch {
      return { ok: false, error: "agent_unreachable" };
    } finally {
      clearTimeout(timer);
    }
    const safeId =
      request.agentId.toLowerCase().replace(/[^a-z0-9]/gu, "").slice(0, 16) ||
      randomBytes(8).toString("hex");
    const serverName = `mode-agent-${safeId}`;
    const name = request.name?.trim() || request.host;
    const config = {
      type: "http",
      url: `http://${request.host}:${request.port}/mcp`,
      headers: { Authorization: `Bearer ${request.token}` },
      timeoutMs: 180_000,
      modeAgent: {
        agentId: request.agentId,
        name,
        host: request.host,
        port: request.port,
        pairedAt: Date.now(),
        ...(request.version ? { version: request.version } : {}),
        ...(request.platform ? { platform: request.platform } : {}),
      },
    };
    try {
      await mcpSync.saveMcpToUserDirectory({
        action: "upsert",
        source: "zcodeagentmcp",
        name: serverName,
        config,
      });
    } catch (error) {
      return {
        ok: false,
        error: error instanceof Error ? error.message : String(error),
      };
    }
    log("enrolled mode agent:", name, serverName);
    return { ok: true, serverName };
  };
}
