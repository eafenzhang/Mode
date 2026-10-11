/** DirectoryBrowser file 模式过滤（spec: remote-workspace-bidirectional-attachments 远端选文件）。
 * 运行：pnpm exec tsx --tsconfig packages/ui/tsconfig.json --test packages/ui/test/directoryBrowserFileMode.test.ts
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  directoryBrowserInteraction,
  selectDirectoryBrowserEntries,
} from "../src/DirectoryBrowser.js";

const entries = [
  { name: "src", path: "/w/src", type: "directory" as const },
  { name: "a.txt", path: "/w/a.txt", type: "file" as const },
];

test("directory 模式只留目录（既有行为）", () => {
  assert.deepEqual(
    selectDirectoryBrowserEntries(entries, "directory").map((entry) => entry.name),
    ["src"],
  );
});

test("file 模式保留目录与文件", () => {
  assert.deepEqual(
    selectDirectoryBrowserEntries(entries, "file").map((entry) => entry.name),
    ["src", "a.txt"],
  );
});

test("只浏览（文件管理器）形态：footer 只有关闭，文件不可选", () => {
  assert.deepEqual(directoryBrowserInteraction("file", true), {
    footer: "close",
    filesSelectable: false,
  });
  // directory + browseOnly 也不会退回「选择当前目录」：只浏览没有可回填语义。
  assert.deepEqual(directoryBrowserInteraction("directory", true), {
    footer: "close",
    filesSelectable: false,
  });
});

test("选择形态不变回归：file 多选确认、directory 选当前目录", () => {
  assert.deepEqual(directoryBrowserInteraction("file", false), {
    footer: "selectFiles",
    filesSelectable: true,
  });
  assert.deepEqual(directoryBrowserInteraction("directory", false), {
    footer: "selectDir",
    filesSelectable: false,
  });
});
