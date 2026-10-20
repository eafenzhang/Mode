import type { ComposerAttachmentUploadItem } from "@/store/composerAttachmentUploadStore.js";

/**
 * 瞬时错误自动重试节奏。远端重连、agent 冷启动和 cold-session-resume 都可能比
 * 一次 500ms 重试更久；只重试一次会让可恢复失败直接落成永久红点。
 * 有界退避覆盖常见恢复窗口，用尽后交给「新连接/新会话」恢复触发点。
 */
const ATTACHMENT_AUTO_RETRY_DELAYS_MS = [500, 1_500, 3_000] as const;

/** 第 attempt 次自动重试的延迟（0-based）；undefined 表示预算用尽。 */
export function attachmentAutoRetryDelayMs(attempt: number): number | undefined {
  return ATTACHMENT_AUTO_RETRY_DELAYS_MS[attempt];
}

/**
 * 可恢复失败：远端断链、连接关闭、会话暂不可用等非确定性错误。
 *
 * permanent 是确定性错误（超限/格式非法/对端不支持），runtimeRestarted 已用尽换代
 * 重传配额；这两类自动恢复只会重复失败或形成循环，必须留给用户手动决定。
 */
export function isRecoverableAttachmentUploadFailure(
  item: Pick<
    ComposerAttachmentUploadItem,
    "referenceOwnership" | "uploadStatus" | "uploadErrorKind"
  >,
): boolean {
  return (
    item.referenceOwnership === "composer" &&
    item.uploadStatus === "failed" &&
    item.uploadErrorKind === "transient"
  );
}

/**
 * 恢复触发点（新会话 / 新远端连接）重新入队的统一字段重置。
 *
 * 保留 runtimeRebuildRetryCount：它限制的是「runtime 反复换代」这一条路径，
 * 自动恢复不能把它清零，否则 Helper 反复崩溃时会无限重传。
 */
export function createRequeuedAttachmentState(
  item: ComposerAttachmentUploadItem,
  status: "queued" | "waitingSession",
): ComposerAttachmentUploadItem {
  return {
    ...item,
    uploadStatus: status,
    uploadProgress: 0,
    uploadError: undefined,
    uploadErrorKind: undefined,
    attachmentRef: undefined,
    autoRetryCount: 0,
    staged: false,
    adopted: false,
    showComplete: false,
  };
}

/** 手动重试是显式用户动作，连换代配额一起清零，保证「再试一次」语义完整。 */
export function createManuallyRetriedAttachmentState(
  item: ComposerAttachmentUploadItem,
  status: "queued" | "waitingSession",
): ComposerAttachmentUploadItem {
  return {
    ...createRequeuedAttachmentState(item, status),
    runtimeRebuildRetryCount: 0,
  };
}
