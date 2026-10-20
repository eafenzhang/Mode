import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

// 全屏图片/视频预览的关闭（docs/specs/m4-ui-foundation.md §2.4）：
// 输入框附件预览挂在 composer 的 will-change: transform 容器内，
// 关闭必须同时满足 portal 与组件自身 onOpenChange 收口。
// packages/ui 没有 DOM 环境，这里用源码契约锁定接线。

const previewSource = await readFile(
  new URL("../src/components/ai-elements/image-preview-dialog.tsx", import.meta.url),
  "utf8",
);
const composerSource = await readFile(
  new URL("../src/v4/ConversationComposer.tsx", import.meta.url),
  "utf8",
);

test("图片/视频预览的关闭按钮与 Esc 直接调用 onOpenChange(false)", () => {
  assert.ok(
    previewSource.includes("onClick={() => onOpenChange(false)}"),
    "关闭按钮必须由预览组件自身收口，不依赖 DialogClose/上下文",
  );
  assert.ok(
    previewSource.includes("onEscapeKeyDown={() => onOpenChange(false)}"),
    "Esc 必须走同一条关闭路径",
  );
  assert.ok(
    !previewSource.includes("<DialogClose"),
    "预览关闭不再经过 adapter 的 DialogClose",
  );
});

test("输入框附件预览接线到 composer 的 open/onOpenChange", () => {
  assert.ok(composerSource.includes("items={composerMediaPreviewItems}"));
  assert.ok(composerSource.includes("onOpenChange={setAttachmentPreviewOpen}"));
  assert.ok(composerSource.includes("open={attachmentPreviewOpen}"));
});
