/** DirectoryBrowser file 模式过滤（spec: remote-workspace-bidirectional-attachments 远端选文件）。
 * 运行：pnpm exec tsx --test packages/ui/test/directoryBrowserFileMode.test.ts
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { selectDirectoryBrowserEntries } from "../src/DirectoryBrowser.js";

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
