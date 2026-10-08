import type { ModeSessionStateSnapshot } from "@mode/shared";
import { createServiceLogger } from "#src/logger/serviceLogger.js";
import { repairImportedClaudeSessionSnapshot } from "#src/session/claude-native/importedClaudeHistoryRepair.js";
import type { IModeAgentService } from "#src/mode-agent/modeAgent.js";
import type {
  ModeSessionReadParams,
  ModeSessionResumeParams,
} from "#src/mode-session/modeSession.js";

const logger = createServiceLogger("mode-session-service");

export async function repairEmptyImportedClaudeSessionSnapshot(params: {
  agentService: IModeAgentService;
  snapshot: ModeSessionStateSnapshot;
  target: ModeSessionResumeParams | ModeSessionReadParams;
}): Promise<ModeSessionStateSnapshot> {
  const repaired = await repairImportedClaudeSessionSnapshot({
    snapshot: params.snapshot,
    target: {
      workspacePath: params.target.workspacePath,
      workspaceIdentity: params.target.workspaceIdentity,
      taskId: params.target.sessionId,
      ...("mcpServers" in params.target && params.target.mcpServers
        ? { mcpServers: params.target.mcpServers }
        : {}),
    },
    createSession: (input) => params.agentService.createSession(input),
    onRepair: (history) => {
      logger.warn(
        undefined,
        `[mode-session-service] Claude 导入 session 历史异常，按 ${history.source} 回填 taskId=${params.target.sessionId}`,
      );
    },
  });
  return repaired ?? params.snapshot;
}
