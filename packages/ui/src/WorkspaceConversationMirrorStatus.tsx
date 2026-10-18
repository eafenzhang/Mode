import { CloudDownload, RefreshCw } from "lucide-react";
import type { IServiceAccessor } from "@mode/services";
import { Button } from "@/ui-foundation/adapters/button.js";
import { useWorkspaceConversationMirror } from "@/hooks/useWorkspaceConversationMirror.js";
import { useWorkspaceServicesResolution } from "@/hooks/useWorkspaceServices.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";

/**
 * 远端工作区对话记录的同步状态行（docs/specs/remote-workspace-conversation-mirror.md）。
 *
 * 镜像会话本身是真会话、直接进任务列表，因此这里**不再渲染记录列表**；只在同步中 / 对端版本过低 /
 * 同步失败时给一行提示与重试入口，避免正常状态下多一行噪音。
 */
export function WorkspaceConversationMirrorStatus({
  workspacePath,
  workspaceIdentity,
  remoteSessionId,
  remoteTarget,
  localServices,
  windowSize,
}: {
  workspacePath: string;
  workspaceIdentity?: string;
  remoteSessionId?: string | null;
  remoteTarget?: unknown;
  localServices: IServiceAccessor;
  /**
   * 侧栏该工作区当前可见的会话条数：镜像同步窗口的基准（实际窗口至少 10 条、至多 20 条，
   * 见 `resolveConversationMirrorWindowSize`），展开更多后随之增长。
   */
  windowSize: number;
}) {
  const { intl } = useModeIntl();
  const resolution = useWorkspaceServicesResolution(
    workspacePath,
    remoteSessionId ?? null,
    workspaceIdentity ?? null,
    remoteTarget,
  );
  const { state, refresh } = useWorkspaceConversationMirror({
    workspacePath,
    ...(workspaceIdentity ? { workspaceIdentity } : {}),
    viewServices: resolution.services,
    localServices,
    isRemoteTarget: resolution.isRemoteTarget,
    remoteSessionId: remoteSessionId ?? null,
    windowSize,
    enabled: resolution.isRemoteTarget && resolution.rpcReady,
  });

  const showSyncing = state.status === "syncing";
  const showUnsupported = state.status === "unsupported";
  const showError = state.status === "error";
  // degraded（有记录但一条都没读成功）必须渲染：它以前被吞进 ready，整轮全面失败的镜像在界面上
  // 完全不可见，用户只会觉得「什么都没同步过来」而拿不到任何线索。
  const showDegraded = state.status === "degraded";
  if (!showSyncing && !showUnsupported && !showError && !showDegraded) {
    return null;
  }

  return (
    <div
      className="mt-1 flex items-center gap-1.5 px-2 pb-1.5 text-ui-xs text-foreground-subtle"
      data-testid="conversation-mirror-status"
    >
      <CloudDownload className="size-3.5 shrink-0" />
      <span className="min-w-0 flex-1 truncate">
        {showUnsupported
          ? intl.formatMessage({ id: "conversationMirror.unsupported" })
          : showError
            ? intl.formatMessage({ id: "conversationMirror.error" })
            : showDegraded
              ? intl.formatMessage({ id: "conversationMirror.degraded" })
              : intl.formatMessage({ id: "conversationMirror.syncing" })}
      </span>
      {showSyncing ? (
        <RefreshCw className="h-3 w-3 shrink-0 animate-spin" />
      ) : (
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          className="shrink-0 text-foreground-subtle hover:text-foreground"
          onClick={refresh}
          aria-label={intl.formatMessage({ id: "conversationMirror.retry" })}
        >
          <RefreshCw className="h-3 w-3" />
        </Button>
      )}
    </div>
  );
}
