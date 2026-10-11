/**
 * 宿主读路径回归：`readPromptAttachment` → decodePromptAttachmentDataUrl 不再按媒体类型拦截。
 * （spec: remote-workspace-bidirectional-attachments 收向「任意 MIME」+ 验收场景 1/7/10）
 * 运行：pnpm --filter @mode/bootstrap test
 *
 * 取舍：`readPromptAttachment` 定义在 `createModeApp` 闭包内，构造完整 App 需要 provider
 * registry/storage/port 装配，无法在单测里直构；因此把判定逻辑抽成纯函数
 * `src/app/prompt-attachment-decode.ts`（读路径唯一出口），对它做端到端语义断言。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { decodePromptAttachmentDataUrl } from "../src/app/prompt-attachment-decode.js";

/** artifact store 存的形态：`data:<mime>;base64,<payload>`，fallbackMime 是权威行的 mime。 */
function artifactDataUrl(mime: string, bytes: Uint8Array): string {
  return `data:${mime};base64,${Buffer.from(bytes).toString("base64")}`;
}

test("text/plain artifact 不再抛 previewNotMedia，按行 mime 返回字节", () => {
  const text = "远端工作区的笔记\n第二行";
  const content = artifactDataUrl("text/plain", Buffer.from(text, "utf8"));
  const result = decodePromptAttachmentDataUrl(content, "text/plain", 1024);
  assert.equal(result.mediaType, "text/plain");
  assert.equal(Buffer.from(result.bytes).toString("utf8"), text);
});

test("image/pdf 仍按原语义解码（媒体不回归）", () => {
  const png = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  const image = decodePromptAttachmentDataUrl(artifactDataUrl("image/png", png), "image/png", 1024);
  assert.equal(image.mediaType, "image/png");
  assert.deepEqual(image.bytes, png);

  const pdf = decodePromptAttachmentDataUrl(
    artifactDataUrl("application/pdf", Buffer.from("%PDF-1.7\n")),
    "application/pdf",
    1024,
  );
  assert.equal(pdf.mediaType, "application/pdf");
});

test("data: 头里的 mime 优先于 fallbackMime；头缺类型时回退 fallbackMime", () => {
  const bare = Buffer.from("note").toString("base64");
  const withHeader = decodePromptAttachmentDataUrl(`data:text/markdown;base64,${bare}`, "text/plain", 1024);
  assert.equal(withHeader.mediaType, "text/markdown");
  const noHeader = decodePromptAttachmentDataUrl(`data:;base64,${bare}`, "text/plain", 1024);
  assert.equal(noHeader.mediaType, "text/plain");
});

test("结构非法（非 base64）仍抛 previewArtifactInvalid", () => {
  assert.throws(
    () => decodePromptAttachmentDataUrl("data:text/plain;base64,%%%not-base64", "text/plain", 1024),
    /fault\.attachment\.previewArtifactInvalid/u,
  );
  assert.throws(
    () => decodePromptAttachmentDataUrl("data:text/plain,hello", "text/plain", 1024),
    /fault\.attachment\.previewArtifactInvalid/u,
  );
});

test("超限仍抛 previewTooLarge（容量语义不变）", () => {
  const content = artifactDataUrl("text/plain", Buffer.alloc(64, 0x61));
  assert.throws(() => decodePromptAttachmentDataUrl(content, "text/plain", 32), (error: unknown) => {
    assert.ok(error instanceof Error);
    assert.equal(error.message, "fault.attachment.previewTooLarge");
    return true;
  });
  // 恰好等于上限不抛。
  assert.doesNotThrow(() => decodePromptAttachmentDataUrl(content, "text/plain", 64));
});
