import { useMemo } from "react";
import { Badge } from "@/components/ui/badge.js";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog.js";
import type { ConversationMirrorDisplayRecord } from "@/lib/conversationMirror.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";

/**
 * 镜像会话的只读查看器：只渲染镜像投影里的角色 + 文本，不带工具/附件/继续对话能力。
 * 记录本身来自对端机器，本机不产生任何会话事实（docs/specs/remote-workspace-conversation-mirror.md）。
 */
export function ConversationMirrorRecordDialog({
  record,
  open,
  onOpenChange,
}: {
  record: ConversationMirrorDisplayRecord | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const { intl, locale } = useModeIntl();
  const formatTime = useMemo(
    () => new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }),
    [locale],
  );

  if (!record) {
    return null;
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-3xl gap-0 overflow-hidden p-0" showCloseButton>
        <DialogHeader className="gap-1 border-b border-popover-border px-4 py-3 pe-12">
          <DialogTitle className="truncate">
            {record.title.trim() ||
              intl.formatMessage({ id: "conversationMirror.untitledRecord" })}
          </DialogTitle>
          <DialogDescription className="flex flex-wrap items-center gap-2 text-ui-base">
            <Badge variant="secondary">
              {intl.formatMessage(
                { id: "conversationMirror.originBadge" },
                { label: record.originLabel },
              )}
            </Badge>
            <span>{formatTime.format(new Date(record.updatedAt))}</span>
            <span>{intl.formatMessage({ id: "conversationMirror.readOnly" })}</span>
            {record.originTruncated ? (
              <span>{intl.formatMessage({ id: "conversationMirror.truncated" })}</span>
            ) : null}
          </DialogDescription>
        </DialogHeader>
        <div className="max-h-[70vh] space-y-3 overflow-y-auto px-4 py-3">
          {record.messages.map((message, index) => (
            <div key={`${index}-${message.role}`} className="space-y-1">
              <div className="text-ui-xs font-medium text-foreground-subtle">
                {message.role === "user"
                  ? intl.formatMessage({ id: "conversationMirror.roleUser" })
                  : intl.formatMessage({ id: "conversationMirror.roleAssistant" })}
              </div>
              <div className="rounded-lg bg-surface-hover/40 px-3 py-2 text-ui-base break-words whitespace-pre-wrap text-foreground">
                {message.text}
              </div>
            </div>
          ))}
        </div>
      </DialogContent>
    </Dialog>
  );
}
