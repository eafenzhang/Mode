import { useCallback, useMemo, useState } from "react";
import { CloudDownload, RefreshCw } from "lucide-react";
import type { IServiceAccessor } from "@mode/services";
import { Button } from "@/components/ui/button.js";
import { Badge } from "@/components/ui/badge.js";
import { cn } from "@/components/lib/utils.js";
import { ConversationMirrorRecordDialog } from "@/ConversationMirrorRecordDialog.js";
import type { ConversationMirrorDisplayRecord } from "@/lib/conversationMirror.js";
import { useWorkspaceConversationMirror } from "@/hooks/useWorkspaceConversationMirror.js";
import { useWorkspaceServicesResolution } from "@/hooks/useWorkspaceServices.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";

/**
 * 工作区侧栏里的「镜像记录」分区：展示对端机器发布到本工作区的只读对话记录
 * （docs/specs/remote-workspace-conversation-mirror.md）。
 *
 * 没有记录且对端支持时整体不渲染，避免给每个工作区都加一行空分区；对端旧版本时给出降级提示。
 */
export function WorkspaceConversationMirrorSection({
  workspacePath,
  workspaceIdentity,
  remoteSessionId,
  remoteTarget,
  isActiveWorkspace,
  localServices,
}: {
  workspacePath: string;
  workspaceIdentity?: string;
  remoteSessionId?: string | null;
  remoteTarget?: unknown;
  /** 本地工作区在激活时才同步（等价于「打开工作区时同步一次」）。 */
  isActiveWorkspace: boolean;
  localServices: IServiceAccessor;
}) {
  const { intl, locale } = useModeIntl();
  const resolution = useWorkspaceServicesResolution(
    workspacePath,
    remoteSessionId ?? null,
    workspaceIdentity ?? null,
    remoteTarget,
  );
  const isRemoteTarget = resolution.isRemoteTarget;
  // 远端 tab 必须用对端代理读写镜像目录；本地 tab 只读/写本机镜像目录。
  const remoteMirrorService = isRemoteTarget
    ? resolution.services.conversationMirrorService
    : undefined;
  const { state, refresh } = useWorkspaceConversationMirror({
    workspacePath,
    ...(workspaceIdentity ? { workspaceIdentity } : {}),
    localServices,
    remoteMirrorService,
    remoteSessionId: remoteSessionId ?? null,
    enabled: isActiveWorkspace || (isRemoteTarget && resolution.rpcReady),
  });
  const [selectedRecord, setSelectedRecord] = useState<ConversationMirrorDisplayRecord | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);

  const formatTime = useMemo(
    () => new Intl.DateTimeFormat(locale, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" }),
    [locale],
  );

  const openRecord = useCallback((record: ConversationMirrorDisplayRecord) => {
    setSelectedRecord(record);
    setDialogOpen(true);
  }, []);

  const hasRecords = state.records.length > 0;
  const showUnsupported = isRemoteTarget && state.status === "unsupported";
  const showError = state.status === "error";
  if (!hasRecords && !showUnsupported && !showError) {
    return null;
  }

  return (
    <div className="mt-1 space-y-1 px-2 pb-2" data-testid="conversation-mirror-section">
      <div className="flex items-center justify-between gap-2 px-1 py-0.5">
        <div className="flex min-w-0 items-center gap-1.5 text-ui-xs font-medium text-foreground-subtle">
          <CloudDownload className="size-3.5 shrink-0" />
          <span className="truncate">
            {intl.formatMessage({ id: "conversationMirror.sectionTitle" })}
          </span>
          {hasRecords ? (
            <span className="shrink-0">
              {intl.formatMessage(
                { id: "conversationMirror.recordCount" },
                { count: state.records.length },
              )}
            </span>
          ) : null}
          {state.status === "syncing" ? (
            <span className="shrink-0">{intl.formatMessage({ id: "conversationMirror.syncing" })}</span>
          ) : null}
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          className="shrink-0 text-foreground-subtle hover:text-foreground"
          onClick={refresh}
          aria-label={intl.formatMessage({ id: "conversationMirror.refresh" })}
        >
          <RefreshCw className={cn("h-3 w-3", state.status === "syncing" && "animate-spin")} />
        </Button>
      </div>

      {showUnsupported ? (
        <div className="px-1 text-ui-xs text-foreground-subtle">
          {intl.formatMessage({ id: "conversationMirror.unsupported" })}
        </div>
      ) : null}
      {showError ? (
        <div className="px-1 text-ui-xs text-destructive">
          {intl.formatMessage({ id: "conversationMirror.error" })}
        </div>
      ) : null}

      {state.records.map((record) => (
        <button
          key={`${record.originDeviceId}:${record.sessionId}`}
          type="button"
          onClick={() => openRecord(record)}
          className="flex w-full min-w-0 items-center gap-2 rounded-md px-1.5 py-1 text-start text-ui-xs text-foreground-subtle transition-colors hover:bg-surface-hover hover:text-foreground"
          title={record.title}
        >
          <span className="min-w-0 flex-1 truncate">
            {record.title.trim() ||
              intl.formatMessage({ id: "conversationMirror.untitledRecord" })}
          </span>
          <Badge variant="outline" className="shrink-0">
            {record.originLabel}
          </Badge>
          <span className="shrink-0 tabular-nums">{formatTime.format(new Date(record.updatedAt))}</span>
        </button>
      ))}

      <ConversationMirrorRecordDialog
        record={selectedRecord}
        open={dialogOpen}
        onOpenChange={setDialogOpen}
      />
    </div>
  );
}
