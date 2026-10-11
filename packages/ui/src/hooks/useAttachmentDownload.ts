import { useCallback, useState } from "react";
import { toast } from "@/components/ui/toast.js";
import { useModeIntl } from "@/i18n/IntlProvider.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { logger } from "@/logger.js";
import type { AttachmentRef } from "@mode/shared/mode-protocol-v4";
import { buildAttachmentDownloadReadParams, downloadAttachment } from "@/lib/attachmentDownload.js";
import type { ConversationRowRenderContext } from "@/v4/conversationRowContext.js";

type ReadAttachmentFn = NonNullable<ConversationRowRenderContext["readAttachment"]>;

export interface AttachmentDownloadInput {
  attachment: Pick<AttachmentRef, "ref" | "fileName" | "mime" | "previewRef">;
  rowId: number;
  entityId?: string;
  attachmentIndices?: readonly number[];
  index: number;
  /** artifact 行等无附件序号场景传 false。 */
  includeAttachmentIndex?: boolean;
}

/** 历史附件下载：读字节 → 平台另存（Electron 原生对话框）/ 浏览器回退；toast 收口结果。 */
export function useAttachmentDownload(options: {
  sessionId?: string;
  readAttachment?: ReadAttachmentFn;
}) {
  const { sessionId, readAttachment } = options;
  const platform = usePlatform();
  const { intl } = useModeIntl();
  const [downloadingRefs, setDownloadingRefs] = useState<ReadonlySet<string>>(() => new Set());

  const triggerBrowserDownload = useCallback(
    (bytes: Uint8Array, mediaType: string, fileName: string) => {
      const blob = new Blob([bytes.slice()], { type: mediaType });
      const objectUrl = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = objectUrl;
      link.download = fileName;
      document.body.appendChild(link);
      link.click();
      link.remove();
      URL.revokeObjectURL(objectUrl);
    },
    [],
  );

  const download = useCallback(
    (input: AttachmentDownloadInput) => {
      if (!sessionId || !readAttachment) return;
      const ref = input.attachment.previewRef ?? input.attachment.ref;
      setDownloadingRefs((current) => new Set(current).add(ref));
      const readParams = buildAttachmentDownloadReadParams({
        attachment: input.attachment,
        sessionId,
        rowId: input.rowId,
        ...(input.entityId ? { entityId: input.entityId } : {}),
        ...(input.attachmentIndices ? { attachmentIndices: input.attachmentIndices } : {}),
        index: input.index,
        ...(input.includeAttachmentIndex !== undefined
          ? { includeAttachmentIndex: input.includeAttachmentIndex }
          : {}),
      });
      void downloadAttachment(
        {
          readAttachment: (params) => readAttachment(params),
          ...(platform.saveFile
            ? {
                // IPlatformService.saveFile 的 canceled 是可选的，SaveFileFn 要求必填；适配层补齐默认值。
                saveFile: async (payload) => {
                  const result = await platform.saveFile!(payload);
                  return { ...result, canceled: result.canceled ?? false };
                },
              }
            : {}),
          triggerBrowserDownload,
        },
        {
          sessionId: readParams.sessionId,
          ref: readParams.ref,
          fileName: input.attachment.fileName,
          ...(readParams.target ? { target: readParams.target } : {}),
          ...(readParams.attachmentIndex !== undefined
            ? { attachmentIndex: readParams.attachmentIndex }
            : {}),
        },
      )
        .then((outcome) => {
          if (outcome.status === "saved") {
            toast(
              intl.formatMessage(
                { id: "chat.attachments.downloadSucceeded" },
                { path: outcome.path },
              ),
            );
            return;
          }
          if (outcome.status === "browserDownloaded") {
            toast(intl.formatMessage({ id: "chat.attachments.downloadStarted" }));
            return;
          }
          if (outcome.status === "canceled") return;
          const failureKey =
            outcome.failure === "tooLarge"
              ? "chat.attachments.downloadTooLarge"
              : outcome.failure === "unauthorized"
                ? "chat.attachments.downloadUnauthorized"
                : outcome.failure === "legacyPeer"
                  ? "chat.attachments.downloadLegacyPeer"
                  : "chat.attachments.downloadFailed";
          // toast 组件无 destructive 变体，warning 是既有失败态文案的统一档位。
          toast(intl.formatMessage({ id: failureKey }), { variant: "warning" });
          logger.warn("[attachment-download] 附件下载失败", {
            ref,
            failure: outcome.failure,
            message: outcome.message,
          });
        })
        .finally(() => {
          setDownloadingRefs((current) => {
            const next = new Set(current);
            next.delete(ref);
            return next;
          });
        });
    },
    [intl, platform, readAttachment, sessionId, triggerBrowserDownload],
  );

  return { download, downloadingRefs };
}
