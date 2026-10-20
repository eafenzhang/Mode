/**
 * 粘贴附件采集的兜底口径。
 *
 * 1. `files` 为空时从 `items` 取文件（部分剪贴板来源只填 items）；
 * 2. 剪贴板截图没有文件名时给一个稳定的兜底名，避免撞 wire schema 的 fileName.min(1)。
 */
import assert from "node:assert/strict";
import test from "node:test";

import { createChatComposerAttachment, readClipboardFiles } from "../src/lib/chatAttachments.js";

function clipboard(payload: {
  files?: File[];
  items?: Array<{ kind: string; getAsFile: () => File | null }>;
}): Pick<DataTransfer, "files" | "items"> {
  return {
    files: payload.files ?? [],
    items: payload.items ?? [],
  } as unknown as Pick<DataTransfer, "files" | "items">;
}

test("files 优先；files 为空时从 items 兜底取文件", () => {
  const fileA = new File(["a"], "a.png", { type: "image/png" });
  const fileB = new File(["b"], "b.png", { type: "image/png" });

  assert.deepEqual(
    readClipboardFiles(
      clipboard({
        files: [fileA],
        items: [{ kind: "file", getAsFile: () => fileB }],
      }),
    ),
    [fileA],
  );
  assert.deepEqual(
    readClipboardFiles(clipboard({ items: [{ kind: "file", getAsFile: () => fileB }] })),
    [fileB],
  );
  assert.deepEqual(
    readClipboardFiles(
      clipboard({
        items: [
          { kind: "string", getAsFile: () => fileB },
          { kind: "file", getAsFile: () => null },
        ],
      }),
    ),
    [],
    "非 file item 与 getAsFile 返回 null 都必须忽略",
  );
});

test("剪贴板文件没有名字时按 MIME 生成兜底文件名", () => {
  const anonymous = new File([new Uint8Array([1, 2, 3])], "", { type: "image/png" });
  const attachment = createChatComposerAttachment(anonymous);
  assert.equal(attachment.filename, "clipboard.png");
  assert.equal(attachment.mimeType, "image/png");

  const named = new File(["x"], "shot.png", { type: "image/png" });
  assert.equal(createChatComposerAttachment(named).filename, "shot.png");
});
