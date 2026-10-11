/**
 * 附件行授权矩阵（spec: remote-workspace-bidirectional-attachments 收向不变量 2）。
 * 运行：pnpm --filter @mode/bootstrap test
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { authorizeConversationAttachment } from "../src/mode-protocol-v4/attachment-row-auth.js";
import type { ConversationRow } from "@mode/shared/mode-protocol-v4";

// 测试夹具只依赖授权读取的字段；zod 全量必填字段与授权无关，用 unknown 断言收窄。
function row(partial: Record<string, unknown>): ConversationRow {
  return {
    rowId: 1,
    turnId: "turn-1",
    entityId: "ent-1",
    createdAt: 0,
    createdAtSeq: 0,
    ...partial,
  } as unknown as ConversationRow;
}

const pdfAttachment = {
  ref: "mode-artifact://s1/a1",
  fileName: "a.pdf",
  mime: "application/pdf",
  bytes: 100,
};
const textAttachment = {
  ref: "/home/u/notes.txt",
  fileName: "notes.txt",
  mime: "text/plain",
  bytes: 10,
  previewRef: "preview/notes.txt",
};

test("target+index：userInput 行 ref 精确匹配，任意 MIME 授权（mediaOnly:false）", () => {
  const rows = [row({ kind: "userInput", attachments: [textAttachment] })];
  const result = authorizeConversationAttachment(rows, {
    sessionId: "s1",
    ref: "/home/u/notes.txt",
    target: { rowId: 1, entityId: "ent-1" },
    attachmentIndex: 0,
    mediaOnly: false,
  });
  assert.ok(result);
  assert.equal(result.attachmentIndex, 0);
  assert.equal(result.sourceRowId, 1);
  assert.equal(result.attachment.fileName, "notes.txt");
});

test("target+index：mediaOnly:true 时非媒体拒绝", () => {
  const rows = [row({ kind: "userInput", attachments: [textAttachment] })];
  const result = authorizeConversationAttachment(rows, {
    sessionId: "s1",
    ref: "/home/u/notes.txt",
    target: { rowId: 1, entityId: "ent-1" },
    attachmentIndex: 0,
    mediaOnly: true,
  });
  assert.equal(result, null);
});

test("target+index：ref 不匹配 / 跨行 entityId 拒绝", () => {
  const rows = [row({ kind: "userInput", attachments: [pdfAttachment] })];
  assert.equal(
    authorizeConversationAttachment(rows, {
      sessionId: "s1",
      ref: "mode-artifact://other/a1",
      target: { rowId: 1, entityId: "ent-1" },
      attachmentIndex: 0,
      mediaOnly: false,
    }),
    null,
  );
  assert.equal(
    authorizeConversationAttachment(rows, {
      sessionId: "s1",
      ref: "mode-artifact://s1/a1",
      target: { rowId: 2, entityId: "ent-1" },
      attachmentIndex: 0,
      mediaOnly: false,
    }),
    null,
  );
});

test("无 target 扫描：userInput 按 previewRef ?? ref 授权（保持旧不对称语义）", () => {
  const rows = [row({ kind: "userInput", attachments: [textAttachment] })];
  const result = authorizeConversationAttachment(rows, {
    sessionId: "s1",
    ref: "preview/notes.txt",
    mediaOnly: false,
  });
  assert.ok(result);
  assert.equal(result.attachmentIndex, undefined);
  assert.equal(result.sourceRowId, undefined);
  assert.equal(
    authorizeConversationAttachment(rows, {
      sessionId: "s1",
      ref: "preview/other.txt",
      mediaOnly: false,
    }),
    null,
  );
});

test("artifact 行：按 ref 授权，行字段合成 AttachmentRef", () => {
  const rows = [
    row({
      kind: "artifact",
      ref: "mode-artifact://s1/run/report.pdf",
      displayName: "report.pdf",
      mimeType: "application/pdf",
      sizeBytes: 2048,
    }),
  ];
  const result = authorizeConversationAttachment(rows, {
    sessionId: "s1",
    ref: "mode-artifact://s1/run/report.pdf",
    target: { rowId: 1, entityId: "ent-1" },
    mediaOnly: false,
  });
  assert.ok(result);
  assert.equal(result.attachment.fileName, "report.pdf");
  assert.equal(result.attachment.mime, "application/pdf");
  assert.equal(result.attachment.bytes, 2048);
  assert.equal(result.attachmentIndex, undefined);
  // 媒体 gate：pdf 属预览媒体（mediaOnly 放行，与旧 isPreviewable 一致）；
  // 非媒体 artifact 在 mediaOnly 下拒绝。
  assert.ok(
    authorizeConversationAttachment(rows, {
      sessionId: "s1",
      ref: "mode-artifact://s1/run/report.pdf",
      mediaOnly: true,
    }),
  );
  assert.equal(
    authorizeConversationAttachment(
      [
        row({
          kind: "artifact",
          ref: "mode-artifact://s1/run/report.bin",
          displayName: "report.bin",
          mimeType: "application/octet-stream",
          sizeBytes: 1,
        }),
      ],
      {
        sessionId: "s1",
        ref: "mode-artifact://s1/run/report.bin",
        mediaOnly: true,
      },
    ),
    null,
  );
});

test("artifact 行：无 target 扫描同样授权", () => {
  const rows = [
    row({
      kind: "artifact",
      ref: "mode-artifact://s1/img.png",
      displayName: "img.png",
      mimeType: "image/png",
      sizeBytes: 5,
    }),
  ];
  const result = authorizeConversationAttachment(rows, {
    sessionId: "s1",
    ref: "mode-artifact://s1/img.png",
    mediaOnly: true,
  });
  assert.ok(result);
});

test("assistantText markdown 图片：session 归属精确匹配才放行", () => {
  const rows = [row({ kind: "assistantText", text: "![x](mode-artifact://s1/img.png)" })];
  const ok = authorizeConversationAttachment(rows, {
    sessionId: "s1",
    ref: "mode-artifact://s1/img.png",
    mediaOnly: false,
  });
  assert.ok(ok);
  assert.equal(ok.sourceRowId, undefined);
  assert.equal(
    authorizeConversationAttachment(rows, {
      sessionId: "s2",
      ref: "mode-artifact://s1/img.png",
      mediaOnly: false,
    }),
    null,
  );
});

test("target 无 index：artifact 行按行身份授权，非匹配 ref 拒绝", () => {
  const artifactRow = row({
    kind: "artifact",
    ref: "mode-artifact://s1/r.bin",
    displayName: "r.bin",
    mimeType: "application/octet-stream",
    sizeBytes: 1,
  });
  const artifactAuth = authorizeConversationAttachment([artifactRow], {
    sessionId: "s1",
    ref: "mode-artifact://s1/r.bin",
    target: { rowId: 1, entityId: "ent-1" },
    mediaOnly: false,
  });
  assert.ok(artifactAuth);
  assert.equal(artifactAuth.attachmentIndex, undefined);
  assert.equal(
    authorizeConversationAttachment([artifactRow], {
      sessionId: "s1",
      ref: "mode-artifact://s1/other",
      target: { rowId: 1, entityId: "ent-1" },
      mediaOnly: false,
    }),
    null,
  );
});
