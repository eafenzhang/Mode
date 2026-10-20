import { memo, useMemo } from "react";
import type { MessageFileLinkTarget } from "@/components/ai-elements/message.js";
import type { CodeViewerSource } from "@/lib/codeViewer.js";
import type { ModeAgentChatSidePaneTab } from "@/lib/workspaceSidePane.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { useMcpStore } from "@/store/mcpStore.js";
import type { PaneWorkspaceScope } from "@/v4/paneLayoutStore.js";
import { SessionPane } from "@/v4/SessionPane.js";
import { V4PaneConversationProvider } from "@/v4/V4ConversationContext.js";

/**
 * 远程控制页右侧的设备对话（docs/specs/remote-control-unified-pairing.md R3）：
 * 顶部固定设备信息，下面复用 SessionPane 的草稿/会话能力。
 */
export const ModeAgentChatSidePane = memo(function ModeAgentChatSidePane({
  tab,
  focused,
  onSessionCreated,
  onSessionDeleted,
  onOpenBrowserUrl,
  onOpenCodeViewer,
  onOpenFileLink,
}: {
  tab: ModeAgentChatSidePaneTab;
  focused: boolean;
  onSessionCreated: (tabId: string, sessionId: string) => void;
  onSessionDeleted: (tabId: string) => void;
  onOpenBrowserUrl?: (url: string) => void;
  onOpenCodeViewer?: (source: CodeViewerSource) => void;
  onOpenFileLink?: (target: MessageFileLinkTarget) => void;
}) {
  const { intl } = useModeIntl();
  const scope = useMemo<PaneWorkspaceScope>(
    () => ({
      workspacePath: tab.workspacePath,
      ...(tab.workspaceIdentity ? { workspaceIdentity: tab.workspaceIdentity } : {}),
      ...(tab.remoteSessionId ? { remoteSessionId: tab.remoteSessionId } : {}),
    }),
    [tab.remoteSessionId, tab.workspaceIdentity, tab.workspacePath],
  );
  const server = useMcpStore((state) =>
    state.servers.find((entry) => entry.name === tab.device.serverName),
  );
  const metadataName = (server?.config as { modeAgent?: { name?: string } } | undefined)?.modeAgent
    ?.name;
  const name = metadataName?.trim() || tab.device.name.trim() || tab.device.serverName;

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      <div className="shrink-0 border-b border-border px-3 py-2">
        <div className="flex min-w-0 items-center gap-2">
          <span className="min-w-0 truncate text-ui-base font-semibold text-foreground">
            {name}
          </span>
          <span className="shrink-0 rounded-full border border-border px-2 py-0.5 text-ui-xs text-foreground-subtle">
            {intl.formatMessage({ id: "remoteDesktop.devices.kind.modeAgent" })}
          </span>
        </div>
        <p className="mt-0.5 truncate font-mono text-ui-xs text-foreground-subtlest">
          {tab.device.host}:{tab.device.port}
        </p>
        <p className="mt-0.5 truncate text-ui-xs text-foreground-subtle">
          {intl.formatMessage(
            { id: "remoteDesktop.modeAgents.mcpInstalled" },
            { server: tab.device.serverName },
          )}
        </p>
      </div>
      <div className="min-h-0 flex-1 overflow-hidden">
        <V4PaneConversationProvider scope={scope}>
          <SessionPane
            paneId={tab.id}
            sessionId={tab.sessionId}
            draftScopeId={`mode-agent-chat:${tab.id}`}
            hideDraftEmptyState
            draftComposerAtBottom
            composerPlaceholder={intl.formatMessage({
              id: "remoteDesktop.modeAgentChat.placeholder",
            })}
            focused={focused}
            workspacePath={tab.workspacePath}
            workspaceIdentity={tab.workspaceIdentity}
            remoteSessionId={tab.remoteSessionId}
            onSessionCreated={(sessionId) => onSessionCreated(tab.id, sessionId)}
            onSessionDeleted={() => onSessionDeleted(tab.id)}
            onOpenBrowserUrl={onOpenBrowserUrl}
            onOpenCodeViewer={onOpenCodeViewer}
            onOpenFileLink={onOpenFileLink}
          />
        </V4PaneConversationProvider>
      </div>
    </div>
  );
});
