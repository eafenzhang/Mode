import { useCallback } from "react";
import type { CreateTaskRequest } from "@/app-shell/types.js";
import type { ModeAgentChatDevice } from "@/lib/workspaceSidePane.js";
import { useServices } from "@/hooks/useServices.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { logger } from "@/logger.js";
import {
  buildModeAgentLaunchPrompt,
  readModeAgentServers,
  renameModeAgentServerConfig,
  type ModeAgentServerRecord,
} from "@/remote-desktop/modeAgentPairing.js";
import { useMcpStore } from "@/store/mcpStore.js";
import { toast } from "@/ui-foundation/adapters/toast.js";

/**
 * 远程控制页的 Mode Agent 设备面：MCP 配置是唯一事实源。
 * 远程 workspace 的 MCP 配置属于对端 Host，这里只返回空列表并跳过刷新。
 */
export function useModeAgentDevices({
  isRemoteWorkspace,
  onCreateTask,
  onOpenModeAgentChat,
}: {
  isRemoteWorkspace: boolean;
  onCreateTask?: (request?: CreateTaskRequest) => void;
  onOpenModeAgentChat?: (request: { device: ModeAgentChatDevice }) => void;
}) {
  const { intl } = useModeIntl();
  const services = useServices();
  const servers = useMcpStore((state) => state.servers);
  const records = isRemoteWorkspace ? [] : readModeAgentServers(servers);

  const refresh = useCallback(async () => {
    if (isRemoteWorkspace) return;
    try {
      const state = useMcpStore.getState();
      await state.loadMcpFromUserDirectory(services.mcpSyncService, state.currentWorkspaceIdentity);
    } catch (error) {
      logger.warn(
        "[remote-desktop] 刷新 Mode Agent 设备失败:",
        error instanceof Error ? error.message : String(error),
      );
    }
  }, [isRemoteWorkspace, services.mcpSyncService]);

  const startChat = useCallback(
    async (record: ModeAgentServerRecord) => {
      const state = useMcpStore.getState();
      const server = state.servers.find((entry) => entry.name === record.serverName);
      if (server && !(state.enabledStates[server.id] ?? server.enabled)) {
        await state.toggleServer(server.id, true);
      }
      if (onOpenModeAgentChat) {
        onOpenModeAgentChat({
          device: {
            serverName: record.serverName,
            agentId: record.agentId,
            name: record.name,
            host: record.host,
            port: record.port,
          },
        });
        return;
      }
      if (!onCreateTask) {
        throw new Error(intl.formatMessage({ id: "remoteDesktop.modeAgents.chatUnavailable" }));
      }
      onCreateTask({ initialPrompt: buildModeAgentLaunchPrompt(record) });
    },
    [intl, onCreateTask, onOpenModeAgentChat],
  );

  const refreshStatus = useCallback(async () => {
    if (isRemoteWorkspace) return;
    const state = useMcpStore.getState();
    const requestEpoch = state.beginServerStatusListRefresh("connect");
    try {
      const result = await services.mcpSyncService.listWorkspaceMcpServerStatuses({
        workspacePath: state.currentProjectPath,
        ...(state.currentWorkspaceIdentity
          ? { workspaceIdentity: state.currentWorkspaceIdentity }
          : {}),
        mcpServers: state.getEnabledMcpServersForMode("mode"),
        mode: "connect",
      });
      useMcpStore.getState().mergeServerStatusSnapshots(result.statuses, requestEpoch, "connect");
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      useMcpStore.getState().markServerStatusListRefreshFailed(message, requestEpoch, "connect");
      throw error;
    }
  }, [isRemoteWorkspace, services.mcpSyncService]);

  const rename = useCallback(
    async (record: ModeAgentServerRecord, name: string) => {
      const trimmed = name.trim();
      if (!trimmed) return;
      const state = useMcpStore.getState();
      const server = state.servers.find((entry) => entry.name === record.serverName);
      if (!server) {
        throw new Error(intl.formatMessage({ id: "remoteDesktop.modeAgents.renameMissing" }));
      }
      await state.updateScopedMcpServer(
        "zcodeagentmcp",
        record.serverName,
        renameModeAgentServerConfig(server.config, record, trimmed),
      );
    },
    [intl],
  );

  const remove = useCallback(
    async (record: ModeAgentServerRecord) => {
      useMcpStore.getState().deleteScopedMcpServer("zcodeagentmcp", record.serverName);
      toast(
        intl.formatMessage({ id: "remoteDesktop.modeAgents.removeSuccess" }, { name: record.name }),
      );
    },
    [intl],
  );

  return { records, refresh, refreshStatus, startChat, rename, remove };
}
