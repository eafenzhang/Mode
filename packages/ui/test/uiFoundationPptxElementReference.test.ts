/**
 * M4：PPTX 元素引用的提示词块与去重（pptx viewer 第二层的前置纯逻辑）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  addPptxElementReference,
  buildPromptWithPptxElementReferences,
  parsePromptPptxElementReferences,
  type PptxElementReference,
} from "../src/lib/pptxElementReference.js";

function reference(overrides: Partial<PptxElementReference> = {}): PptxElementReference {
  return {
    id: "ref-1",
    slideIndex: 0,
    slidePart: "ppt/slides/slide1.xml",
    nodeId: "node-1",
    nodeName: "Title",
    nodeType: "shape",
    bounds: { x: 0, y: 0, width: 100, height: 40 },
    zIndex: 1,
    workspacePath: "/ws",
    sourcePath: "/ws/a.pptx",
    sourceTitle: "a.pptx",
    sourceFingerprint: `sha256:${"a".repeat(64)}`,
    capturedAt: 1,
    ...overrides,
  };
}

test("元素引用去重：同 key 替换，不同 key 追加", () => {
  const first = reference({ comment: "old" });
  const replaced = addPptxElementReference([first], reference({ comment: "new" }));
  assert.equal(replaced.length, 1);
  assert.equal(replaced[0]?.comment, "new");

  const appended = addPptxElementReference([first], reference({ id: "ref-2", nodeId: "node-2" }));
  assert.equal(appended.length, 2);
});

test("提示词块：无引用时只返回 trim 后的文本", () => {
  assert.equal(buildPromptWithPptxElementReferences("  hello  ", []), "hello");
});

test("提示词块：构建后能被解析回原文与引用", () => {
  const prompt = buildPromptWithPptxElementReferences("请看这个元素", [reference()]);
  const parsed = parsePromptPptxElementReferences(prompt);
  assert.equal(parsed.visibleContent, "请看这个元素");
  assert.equal(parsed.pptxElementReferences.length, 1);
  assert.equal(parsed.pptxElementReferences[0]?.nodeId, "node-1");
});

test("提示词块：非法 JSON 保持原文且不产出引用", () => {
  const content = "正文\n\n```json\n{bad}\n```";
  const parsed = parsePromptPptxElementReferences(content);
  assert.equal(parsed.visibleContent, content);
  assert.deepEqual(parsed.pptxElementReferences, []);
});
