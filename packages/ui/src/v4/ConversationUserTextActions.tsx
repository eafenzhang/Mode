import { memo, useCallback, useState } from "react";
import { CheckIcon, CopyIcon, PencilIcon } from "lucide-react";
import { TID_V4_EDIT, testId } from "@mode/shared";
import { MessageAction, MessageActions } from "@/components/ai-elements/message.js";
import { cn } from "@/components/lib/utils.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { formatMessageTimeLabel } from "@/v4/messageTimeLabel.js";

/** 复制行文本：图标 ghost 按钮 + 1200ms 打勾态（对齐旧版 message copy）。
 *  label/tooltip 必须由调用方传入 i18n 文案，禁止硬编码语言。 */
export const CopyRowAction = memo(function CopyRowAction({
  text,
  rowId,
  label,
  tooltip = label,
}: {
  text: string;
  rowId: number;
  label: string;
  tooltip?: string;
}) {
  const [copied, setCopied] = useState(false);
  const handleCopy = useCallback(() => {
    if (!text || !navigator.clipboard) return;
    void navigator.clipboard.writeText(text).then(() => {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1200);
    });
  }, [text]);
  return (
    <MessageAction
      aria-label={label}
      label={label}
      tooltip={tooltip}
      data-testid={`v4-copy-${rowId}`}
      disabled={text.length === 0}
      onClick={handleCopy}
    >
      {copied ? <CheckIcon className="size-3.5 text-success" /> : <CopyIcon className="size-3.5" />}
    </MessageAction>
  );
});

/**
 * 用户消息动作行。动作与时间始终可见：远控/触屏没有 hover，不能把复制、编辑入口
 * 藏在 `group-hover` 后面；时间只从 `row.createdAt` 派生，不新增状态。
 */
export const ConversationUserTextActions = memo(function ConversationUserTextActions({
  rowId,
  text,
  createdAt,
  canEdit,
  onEdit,
  className,
}: {
  rowId: number;
  text: string;
  createdAt: number;
  canEdit: boolean;
  onEdit?: () => void;
  className?: string;
}) {
  const { intl, locale } = useModeIntl();
  const copyLabel = intl.formatMessage({ id: "chat.message.copy" });
  const editLabel = intl.formatMessage({ id: "chat.message.edit" });
  const timeLabel = formatMessageTimeLabel(createdAt, locale, intl);
  return (
    <MessageActions className={cn("mt-1", className)}>
      <CopyRowAction text={text} rowId={rowId} label={copyLabel} />
      {canEdit && onEdit ? (
        <MessageAction
          label={editLabel}
          tooltip={editLabel}
          data-testid={testId(TID_V4_EDIT, String(rowId))}
          onClick={onEdit}
        >
          <PencilIcon className="size-3.5" />
        </MessageAction>
      ) : null}
      {timeLabel ? (
        <span className="select-none text-ui-sm text-foreground-subtlest">{timeLabel}</span>
      ) : null}
    </MessageActions>
  );
});
