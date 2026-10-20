import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import {
  buildEditCodeViewerSource,
  buildEditFileViewerSource,
} from "../src/ToolCallBlocks/renderers/edit.js";
import type { RawToolCallFileSummary } from "../src/ToolCallBlocks/fileSummaryTypes.js";

// 工具卡文件入口的右侧预览目标（docs/specs/tool-preview-source.md）：
// 内联预览继续显示 diff，但点文件 chip 打开右侧时的 source 必须是 file，
// 否则用户点完仍停在绿色新增行上，要再点一次「查看源码」。

const editSource = await readFile(
  new URL("../src/ToolCallBlocks/renderers/edit.tsx", import.meta.url),
  "utf8",
);
const changesGroupSource = await readFile(
  new URL("../src/ToolCallBlocks/renderers/changes-group.tsx", import.meta.url),
  "utf8",
);

const summaryWithPatch: RawToolCallFileSummary = {
  path: "/workspace/a.json",
  actionLabel: "Created",
  operationKind: "write",
  fileName: "a.json",
  filePath: "workspace",
  fileIconSrc: "",
  patch: "@@ -0,0 +1 @@\n+{}",
};

test("Edit/Write 文件入口：内联保留 patch，右侧打开 file source", () => {
  const inlineSource = buildEditCodeViewerSource(summaryWithPatch);
  assert.equal(inlineSource.type, "patch", "折叠态内联预览仍要显示 diff");

  const viewerSource = buildEditFileViewerSource(summaryWithPatch);
  assert.deepEqual(viewerSource, {
    type: "file",
    title: "a.json",
    path: "/workspace/a.json",
  });
});

test("无 patch 的文件摘要两条路径都打开 file source", () => {
  const summary = { ...summaryWithPatch, patch: null };
  assert.equal(buildEditCodeViewerSource(summary).type, "file");
  assert.equal(buildEditFileViewerSource(summary).type, "file");
});

test("Edit / Changes 卡片的点击路径只打开 file source", () => {
  assert.ok(
    editSource.includes("onOpenCodeViewer(buildEditFileViewerSource(summary))"),
    "Edit/Write 文件 chip 必须打开文件源码",
  );
  assert.ok(
    !editSource.includes("onOpenCodeViewer(buildEditCodeViewerSource(summary))"),
    "点击路径不得再直接打开 patch",
  );
  assert.ok(
    changesGroupSource.includes("buildEditFileViewerSource(summary)"),
    "Changes 分组卡的文件 chip 必须打开文件源码",
  );
  assert.ok(
    !changesGroupSource.includes("buildEditCodeViewerSource(summary)"),
    "Changes 分组卡的点击路径不得直接打开 patch",
  );
});
