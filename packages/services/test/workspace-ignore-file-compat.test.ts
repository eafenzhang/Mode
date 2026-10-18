import assert from "node:assert/strict";
import test from "node:test";

import {
  pickWorkspaceIgnoreFileName,
  WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAME,
  WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAMES,
} from "../src/file/workspaceFileIgnore.js";

// 忽略文件改名（.zcodeignore → .modeignore → .tuttoignore）后的选择口径：
// 新名优先，旧名依次兜底；都不存在时返回新名（新建一律用新名）。
test("忽略文件名：三代兼容清单与优先级", () => {
  assert.equal(WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAME, ".tuttoignore");
  assert.deepEqual(
    [...WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAMES],
    [".tuttoignore", ".modeignore", ".zcodeignore"],
  );

  assert.equal(
    pickWorkspaceIgnoreFileName((name) => name === ".tuttoignore"),
    ".tuttoignore",
  );
  assert.equal(
    pickWorkspaceIgnoreFileName((name) => name === ".modeignore"),
    ".modeignore",
  );
  assert.equal(
    pickWorkspaceIgnoreFileName((name) => name === ".zcodeignore"),
    ".zcodeignore",
  );
  assert.equal(
    pickWorkspaceIgnoreFileName(() => false),
    ".tuttoignore",
  );
  // 新名存在时即使旧名也在，仍选新名。
  assert.equal(
    pickWorkspaceIgnoreFileName((name) => name === ".tuttoignore" || name === ".modeignore"),
    ".tuttoignore",
  );
});
