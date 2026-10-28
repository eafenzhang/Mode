/**
 * Web/fpk 临时文本附件（对应桌面 createTempTextAttachment）。
 * 文件名语义必须与 Tauri 命令一致，否则同一段文本在桌面/浏览器上的附件名不同。
 */
import assert from "node:assert/strict";
import { mkdtemp, readFile, readdir, rm, stat, utimes } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  createTempTextAttachment,
  sanitizeTempAttachmentFilename,
  sweepTempAttachments,
  TEMP_ATTACHMENT_DEFAULT_FILENAME,
} from "../src/tempAttachments.js";

test("文件名只保留最后一段，并替换非法字符", () => {
  assert.equal(sanitizeTempAttachmentFilename("a/b/c.txt"), "c.txt");
  assert.equal(sanitizeTempAttachmentFilename("a\\b\\c.txt"), "c.txt");
  assert.match(sanitizeTempAttachmentFilename('bad<>:"|?*.txt'), /^bad_+\.txt$/u);
  assert.equal(sanitizeTempAttachmentFilename("  trailing. . "), "trailing");
});

test("空名 / . / .. 回退到 attachment.txt", () => {
  assert.equal(sanitizeTempAttachmentFilename(undefined), TEMP_ATTACHMENT_DEFAULT_FILENAME);
  assert.equal(sanitizeTempAttachmentFilename(""), TEMP_ATTACHMENT_DEFAULT_FILENAME);
  assert.equal(sanitizeTempAttachmentFilename("."), TEMP_ATTACHMENT_DEFAULT_FILENAME);
  assert.equal(sanitizeTempAttachmentFilename(".."), TEMP_ATTACHMENT_DEFAULT_FILENAME);
});

test("写入唯一子目录，大小按 UTF-8 字节计算", async () => {
  const root = await mkdtemp(join(tmpdir(), "mode-temp-attachment-"));
  try {
    const first = await createTempTextAttachment(root, { text: "你好", filename: "note.txt" });
    const second = await createTempTextAttachment(root, { text: "你好", filename: "note.txt" });
    assert.equal(first.filename, "note.txt");
    assert.equal(first.sizeBytes, Buffer.byteLength("你好", "utf8"));
    assert.equal(first.mimeType, "text/plain");
    assert.notEqual(first.localPath, second.localPath);
    assert.equal(await readFile(first.localPath, "utf8"), "你好");
    assert.equal((await stat(first.localPath)).isFile(), true);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("定期清理：过期目录删除，新目录保留", async () => {
  const root = await mkdtemp(join(tmpdir(), "mode-temp-attachment-sweep-"));
  try {
    const old = await createTempTextAttachment(root, { text: "old", filename: "old.txt" });
    const fresh = await createTempTextAttachment(root, { text: "new", filename: "new.txt" });
    const oldDir = join(old.localPath, "..");
    const eightDaysAgo = new Date(Date.now() - 8 * 24 * 60 * 60 * 1000);
    await utimes(oldDir, eightDaysAgo, eightDaysAgo);

    const result = await sweepTempAttachments(root);
    assert.equal(result.removed, 1);
    assert.equal(result.removedBytes, 3);
    assert.equal((await readdir(root)).length, 1);
    assert.equal(await readFile(fresh.localPath, "utf8"), "new");
    assert.equal(
      await stat(old.localPath).then(
        () => true,
        () => false,
      ),
      false,
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("总量超限：按最旧优先删除", async () => {
  const root = await mkdtemp(join(tmpdir(), "mode-temp-attachment-cap-"));
  try {
    const old = await createTempTextAttachment(root, { text: "12345678", filename: "old.txt" });
    const fresh = await createTempTextAttachment(root, { text: "12345678", filename: "new.txt" });
    const oldDir = join(old.localPath, "..");
    const older = new Date(Date.now() - 60 * 60 * 1000);
    await utimes(oldDir, older, older);

    const result = await sweepTempAttachments(root, { maxTotalBytes: 10 });
    assert.equal(result.removed, 1);
    assert.equal(result.remainingBytes, 8);
    assert.equal((await readdir(root)).length, 1);
    assert.equal(await readFile(fresh.localPath, "utf8"), "12345678");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
