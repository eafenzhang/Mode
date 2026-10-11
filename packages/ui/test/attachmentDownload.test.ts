/** 附件下载纯逻辑（spec: remote-workspace-bidirectional-attachments 收向下载）。
 * 运行：pnpm exec tsx --test packages/ui/test/attachmentDownload.test.ts
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  buildAttachmentDownloadReadParams,
  classifyAttachmentDownloadError,
  downloadAttachment,
  suggestedAttachmentDownloadName,
} from "../src/lib/attachmentDownload.js";

test("classifyAttachmentDownloadError：稳定码 → 分类", () => {
  assert.equal(
    classifyAttachmentDownloadError(new Error("fault.attachment.previewTooLarge")),
    "tooLarge",
  );
  assert.equal(
    classifyAttachmentDownloadError(new Error("fault.attachment.previewRefNotAuthorized")),
    "unauthorized",
  );
  assert.equal(
    classifyAttachmentDownloadError(new Error("fault.attachment.previewNotMedia")),
    "legacyPeer",
  );
  assert.equal(
    classifyAttachmentDownloadError(new Error("fault.attachment.readUnsupported")),
    "unsupported",
  );
  assert.equal(classifyAttachmentDownloadError(new Error("socket hang up")), "unknown");
});

test("suggestedAttachmentDownloadName：去路径分隔符与控制字符，空值回退", () => {
  assert.equal(suggestedAttachmentDownloadName("报表 2024.pdf"), "报表 2024.pdf");
  assert.equal(suggestedAttachmentDownloadName("a/b\\c.txt"), "abc.txt");
  assert.equal(suggestedAttachmentDownloadName(""), "attachment");
});

test("buildAttachmentDownloadReadParams：previewRef 优先 + entityId 才带 target/index", () => {
  const attachment = {
    ref: "/remote/notes.txt",
    previewRef: "preview/notes.txt",
    fileName: "notes.txt",
    mime: "text/plain",
    bytes: 1,
  };
  const withEntity = buildAttachmentDownloadReadParams({
    attachment,
    sessionId: "s1",
    rowId: 7,
    entityId: "ent-7",
    attachmentIndices: [3],
    index: 0,
  });
  assert.equal(withEntity.ref, "preview/notes.txt");
  assert.deepEqual(withEntity.target, { rowId: 7, entityId: "ent-7" });
  assert.equal(withEntity.attachmentIndex, 3);

  const withoutEntity = buildAttachmentDownloadReadParams({
    attachment,
    sessionId: "s1",
    rowId: 7,
    index: 0,
  });
  assert.equal(withoutEntity.target, undefined);
  assert.equal(withoutEntity.attachmentIndex, undefined);
});

test("buildAttachmentDownloadReadParams：artifact 行（includeAttachmentIndex:false）只带 target", () => {
  const params = buildAttachmentDownloadReadParams({
    attachment: {
      ref: "mode-artifact://s1/r.pdf",
      fileName: "r.pdf",
      mime: "application/pdf",
      bytes: 9,
    },
    sessionId: "s1",
    rowId: 12,
    entityId: "ent-12",
    index: 0,
    includeAttachmentIndex: false,
  });
  assert.deepEqual(params.target, { rowId: 12, entityId: "ent-12" });
  assert.equal(params.attachmentIndex, undefined);
});

test("downloadAttachment：saveFile 成功 / 取消 / 大文件错误分类", async () => {
  const bytes = new Uint8Array([1, 2, 3]);
  const readAttachment = async () => ({ bytes, mediaType: "text/plain" });
  const saved: Array<{ data: ArrayBuffer; suggestedName: string }> = [];
  const outcome = await downloadAttachment(
    {
      readAttachment,
      saveFile: async (payload) => {
        saved.push(payload);
        return { canceled: false, success: true, path: "D:/tmp/notes.txt" };
      },
    },
    { sessionId: "s1", ref: "preview/notes.txt", fileName: "notes.txt" },
  );
  assert.equal(outcome.status, "saved");
  assert.equal(saved.length, 1);
  assert.equal(saved[0].suggestedName, "notes.txt");

  const canceled = await downloadAttachment(
    { readAttachment, saveFile: async () => ({ canceled: true, success: false }) },
    { sessionId: "s1", ref: "r", fileName: "n" },
  );
  assert.equal(canceled.status, "canceled");

  const tooLarge = await downloadAttachment(
    {
      readAttachment: async () => {
        throw new Error("fault.attachment.previewTooLarge");
      },
      saveFile: async () => ({ canceled: true, success: false }),
    },
    { sessionId: "s1", ref: "r", fileName: "n" },
  );
  assert.equal(tooLarge.status, "failed");
  assert.equal(tooLarge.status === "failed" ? tooLarge.failure : "", "tooLarge");
});

test("downloadAttachment：无 saveFile 走浏览器下载回调", async () => {
  const triggered: string[] = [];
  const outcome = await downloadAttachment(
    {
      readAttachment: async () => ({ bytes: new Uint8Array([9]), mediaType: "application/pdf" }),
      triggerBrowserDownload: (_bytes, _mediaType, name) => {
        triggered.push(name);
      },
    },
    { sessionId: "s1", ref: "r", fileName: "a.pdf" },
  );
  assert.equal(outcome.status, "browserDownloaded");
  assert.deepEqual(triggered, ["a.pdf"]);
});
