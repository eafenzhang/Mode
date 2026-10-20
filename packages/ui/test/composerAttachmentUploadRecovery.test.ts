/** 远端附件上传失败恢复策略（spec: remote-workspace-bidirectional-attachments 失败恢复）。
 * 运行：pnpm exec tsx --tsconfig packages/ui/tsconfig.json --test packages/ui/test/composerAttachmentUploadRecovery.test.ts
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import type { ComposerAttachmentUploadItem } from "../src/store/composerAttachmentUploadStore.js";
import {
  attachmentAutoRetryDelayMs,
  createManuallyRetriedAttachmentState,
  createRequeuedAttachmentState,
  isRecoverableAttachmentUploadFailure,
} from "../src/v4/composer/attachmentUploadRecovery.js";

function item(overrides: Partial<ComposerAttachmentUploadItem> = {}): ComposerAttachmentUploadItem {
  return {
    id: "att-1",
    filename: "shot.png",
    mimeType: "image/png",
    sizeBytes: 1024,
    referenceOwnership: "composer",
    uploadStatus: "failed",
    uploadProgress: 42,
    uploadError: "fault.connection.closed",
    uploadErrorKind: "transient",
    attachmentRef: { ref: "artifact-1", fileName: "shot.png", mime: "image/png", bytes: 1024 },
    operationId: "prompt-attachment-att-1",
    autoRetryCount: 1,
    runtimeRebuildRetryCount: 2,
    staged: true,
    adopted: true,
    showComplete: true,
    localZeroCopy: false,
    ...overrides,
  };
}

test("只有 composer-owned 的 transient failed 才可自动恢复", () => {
  assert.equal(isRecoverableAttachmentUploadFailure(item()), true);
  assert.equal(isRecoverableAttachmentUploadFailure(item({ uploadErrorKind: "permanent" })), false);
  assert.equal(
    isRecoverableAttachmentUploadFailure(item({ uploadErrorKind: "runtimeRestarted" })),
    false,
  );
  assert.equal(
    isRecoverableAttachmentUploadFailure(item({ referenceOwnership: "session" })),
    false,
  );
  assert.equal(isRecoverableAttachmentUploadFailure(item({ uploadStatus: "ready" })), false);
  assert.equal(
    isRecoverableAttachmentUploadFailure(item({ uploadStatus: "waitingSession" })),
    false,
  );
});

test("瞬时错误按有界退避重试，预算用尽后不再自动重试", () => {
  assert.equal(attachmentAutoRetryDelayMs(0), 500);
  assert.equal(attachmentAutoRetryDelayMs(1), 1_500);
  assert.equal(attachmentAutoRetryDelayMs(2), 3_000);
  assert.equal(attachmentAutoRetryDelayMs(3), undefined);
});

test("自动重传重置上传态与 autoRetry，但保留换代重传配额", () => {
  const next = createRequeuedAttachmentState(item(), "queued");
  assert.equal(next.uploadStatus, "queued");
  assert.equal(next.uploadProgress, 0);
  assert.equal(next.uploadError, undefined);
  assert.equal(next.uploadErrorKind, undefined);
  assert.equal(next.attachmentRef, undefined);
  assert.equal(next.autoRetryCount, 0);
  // runtimeRebuildRetryCount 是防 Helper 反复崩溃的独立配额，自动恢复不能清零。
  assert.equal(next.runtimeRebuildRetryCount, 2);
  assert.equal(next.staged, false);
  assert.equal(next.adopted, false);
  assert.equal(next.showComplete, false);
  assert.equal(next.id, "att-1");
  assert.equal(next.filename, "shot.png");
});

test("waitingSession 恢复同样可用，且不改写归属与来源字段", () => {
  const next = createRequeuedAttachmentState(item({ id: "att-2" }), "waitingSession");
  assert.equal(next.uploadStatus, "waitingSession");
  assert.equal(next.referenceOwnership, "composer");
  assert.equal(next.localZeroCopy, false);
  assert.equal(next.id, "att-2");
});

test("手动重试是显式动作：两项重试计数一起清零", () => {
  const next = createManuallyRetriedAttachmentState(item(), "queued");
  assert.equal(next.uploadStatus, "queued");
  assert.equal(next.autoRetryCount, 0);
  assert.equal(next.runtimeRebuildRetryCount, 0);
  assert.equal(next.uploadError, undefined);
});
