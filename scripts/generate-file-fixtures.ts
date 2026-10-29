/**
 * 生成 `file` 域的 Rust 对拍数据。
 *
 * 真源：
 * - `packages/shared/src/workspaceFileEntriesCodec.ts`
 * - `packages/shared/src/workspaceFileSearch.ts`
 * - `packages/services/src/file/workspaceFileMentionFilter.ts`
 * - `packages/services/src/file/workspaceFileIgnore.ts`
 * - `packages/services/src/file/fileService.ts`（整树扫描）
 *
 * ```bash
 * pnpm file:fixtures
 * cargo test -p file-service
 * ```
 */
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { packWorkspaceFileEntries } from "../packages/shared/src/workspaceFileEntriesCodec.js";
import {
  filterWorkspaceFileSearchCandidates,
  mapWorkspaceFileEntriesToSearchCandidates,
} from "../packages/shared/src/workspaceFileSearch.js";
import { defaultWorkspaceFileSearchFilter } from "../packages/services/src/file/workspaceFileMentionFilter.js";
import {
  loadWorkspaceFileSearchIgnoreRules,
  readWorkspaceFileSearchIgnore,
  transformWorkspaceFileSearchIgnore,
  WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAME,
} from "../packages/services/src/file/workspaceFileIgnore.js";
import { createFileService } from "../packages/services/src/file/fileService.js";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT, "crates", "file-service", "fixtures", "file-fixtures.json");

type EntryType = "file" | "directory";
interface Entry {
  name: string;
  path: string;
  relativePath: string;
  type: EntryType;
}

function entry(relativePath: string, type: EntryType): Entry {
  const name = relativePath.split("/").at(-1)!;
  return { name, path: `/ws/${relativePath}`, relativePath, type };
}

const codecEntries: Entry[] = [
  entry("src", "directory"),
  entry("src/a.ts", "file"),
  entry("src/with\ttab.ts", "file"),
  entry("src/with\nnewline.ts", "file"),
  entry("docs/back\\slash.md", "file"),
  entry("中文/说明.md", "file"),
];
const codecPacked = packWorkspaceFileEntries(codecEntries);

const codec = {
  entries: codecEntries,
  packed: codecPacked,
  unpackPosix: codecEntries.map((item) => ({
    name: item.name,
    path: `/ws/${item.relativePath}`,
    relativePath: item.relativePath,
    type: item.type,
  })),
  unpackWindows: codecEntries.map((item) => ({
    name: item.name,
    path: `C:\\ws\\${item.relativePath.replace(/\//g, "\\")}`,
    relativePath: item.relativePath,
    type: item.type,
  })),
};

const searchEntries: Entry[] = [
  entry("main", "directory"),
  entry("src/main.ts", "file"),
  entry("src/main.test.ts", "file"),
  entry("app/main.ts", "file"),
  entry("docs/readme.md", "file"),
  entry("README.md", "file"),
];
const searchCandidates = mapWorkspaceFileEntriesToSearchCandidates(searchEntries);
const search = {
  entries: searchEntries,
  queries: [
    { query: "main", limit: 3, requireQuery: false },
    { query: "mt", limit: 5, requireQuery: false },
    { query: "  ", limit: 2, requireQuery: false },
    { query: "", limit: 10, requireQuery: true },
    { query: "zzz", limit: 10, requireQuery: false },
  ].map((item) => ({
    ...item,
    expected: filterWorkspaceFileSearchCandidates(searchCandidates, item.query, {
      limit: item.limit,
      requireQuery: item.requireQuery,
    }).map((candidate) => candidate.relativePath),
  })),
};

const filterCases = [
  { name: "node_modules", relativePath: "node_modules", type: "directory", ignoreRulesActive: true },
  { name: "node_modules", relativePath: "node_modules", type: "directory", ignoreRulesActive: false },
  { name: ".github", relativePath: ".github", type: "directory", ignoreRulesActive: true },
  { name: "workflows", relativePath: ".github/workflows", type: "directory", ignoreRulesActive: true },
  { name: "a.dll", relativePath: "lib/a.dll", type: "file", ignoreRulesActive: true },
  { name: ".env.local", relativePath: ".env.local", type: "file", ignoreRulesActive: true },
  { name: "ok.rs", relativePath: "src/ok.rs", type: "file", ignoreRulesActive: true },
  { name: "cmake-build-debug", relativePath: "cmake-build-debug", type: "directory", ignoreRulesActive: false },
] as const;
const filter = {
  cases: filterCases.map((item) => ({
    ...item,
    decision: defaultWorkspaceFileSearchFilter.evaluate(
      { name: item.name, path: `/ws/${item.relativePath}`, relativePath: item.relativePath, type: item.type },
      { ignoreRulesActive: item.ignoreRulesActive },
    ),
  })),
};

const rulesContent = [
  "# comment",
  "node_modules/",
  "*.log",
  "!keep.log",
  "/root-only.txt",
  "build/**",
  "docs/",
  "",
].join("\n");
const matchCases = [
  { path: "node_modules", type: "directory" as const },
  { path: "node_modules/pkg", type: "directory" as const },
  { path: "src/app.log", type: "file" as const },
  { path: "src/keep.log", type: "file" as const },
  { path: "root-only.txt", type: "file" as const },
  { path: "nested/root-only.txt", type: "file" as const },
  { path: "build/x/y.js", type: "file" as const },
  { path: "docs", type: "directory" as const },
  { path: "src/app.ts", type: "file" as const },
];

const tempRoots: string[] = [];
function tempRoot(label: string): string {
  const root = mkdtempSync(path.join(tmpdir(), `mode-file-fixture-${label}-`));
  tempRoots.push(root);
  return root;
}

try {
  // 规则匹配 + 读取/加载来源
  const rulesRoot = tempRoot("rules");
  writeFileSync(path.join(rulesRoot, WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAME), rulesContent);
  const loaded = await loadWorkspaceFileSearchIgnoreRules(rulesRoot);
  const readExisting = await readWorkspaceFileSearchIgnore(rulesRoot);
  const matcher = (await import("ignore")).default().add(rulesContent);
  const ignoreSection = {
    rulesContent,
    matchCases: matchCases.map((item) => ({
      ...item,
      ignored: matcher.ignores(item.type === "directory" ? `${item.path}/` : item.path),
    })),
    readExisting,
    loadSourceExisting: loaded.source,
  };

  // 从 .gitignore 自动创建
  const gitignoreRoot = tempRoot("gitignore");
  const gitignoreContent = "dist/\ncustom/\n*.tmp\n";
  writeFileSync(path.join(gitignoreRoot, ".gitignore"), gitignoreContent);
  const gitignoreLoaded = await loadWorkspaceFileSearchIgnoreRules(gitignoreRoot);
  const readFromGitignore = await readWorkspaceFileSearchIgnore(gitignoreRoot);
  const createdFromGitignore = {
    gitignoreContent,
    loadSource: gitignoreLoaded.source,
    read: readFromGitignore,
  };

  // 无任何文件：内置模板
  const emptyRoot = tempRoot("empty");
  const emptyLoaded = await loadWorkspaceFileSearchIgnoreRules(emptyRoot);
  const readFromTemplate = await readWorkspaceFileSearchIgnore(emptyRoot);
  const createdFromTemplate = {
    loadSource: emptyLoaded.source,
    read: readFromTemplate,
  };

  // sync / reset：用真实 transform（私有纯函数不导出，只能经文件驱动）
  const syncCases = [];
  const resetCases = [];
  const currentContents = [
    ["dist/\ncustom/\n", "build/\n"],
    ["", ""],
    ["not-a-template\n", "dist/\n"],
  ] as const;
  for (const [index, [current, gitignore]] of currentContents.entries()) {
    const syncRoot = tempRoot(`sync-${index}`);
    writeFileSync(path.join(syncRoot, WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAME), current);
    writeFileSync(path.join(syncRoot, ".gitignore"), gitignore);
    const resetRoot = tempRoot(`reset-${index}`);
    writeFileSync(path.join(resetRoot, WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAME), current);
    writeFileSync(path.join(resetRoot, ".gitignore"), gitignore);
    syncCases.push({
      current,
      gitignore,
      expected: (await transformWorkspaceFileSearchIgnore(syncRoot, "sync-gitignore")).content,
    });
    resetCases.push({
      current,
      gitignore,
      expected: (await transformWorkspaceFileSearchIgnore(resetRoot, "reset-defaults")).content,
    });
  }
  const transformRoot = tempRoot("transform");
  writeFileSync(path.join(transformRoot, ".gitignore"), "dist/\n");
  writeFileSync(path.join(transformRoot, WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAME), "custom-rule\n");
  const transformNoMarkers = await transformWorkspaceFileSearchIgnore(transformRoot, "sync-gitignore");

  // 整树扫描：小写 ASCII 名称，避开 localeCompare 差异
  const treeRoot = tempRoot("tree");
  const treeFiles = [
    "README.md",
    ".gitignore",
    "src/main.ts",
    "src/util.ts",
    "logs/app.log",
    "logs/keep.log",
    ".github/workflows/ci.yml",
    "node_modules/pkg/index.js",
    "ignored-dir/a.ts",
  ];
  const treeDirs = ["src", "logs", "ignored-dir", ".github", ".github/workflows", "node_modules", "node_modules/pkg"];
  const treeIgnore = ["node_modules/", "*.log", "!keep.log", "ignored-dir/", ""].join("\n");
  for (const dir of treeDirs) mkdirSync(path.join(treeRoot, dir), { recursive: true });
  for (const file of treeFiles) writeFileSync(path.join(treeRoot, file), "");
  writeFileSync(path.join(treeRoot, WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAME), treeIgnore);

  const fileService = createFileService();
  const length = await fileService.listWorkspaceFilesLength({ rootPath: treeRoot });
  const packed = await fileService.listWorkspaceFilesRange({
    rootPath: treeRoot,
    offset: 0,
    length: 10_000_000,
  });

  const fixtures = {
    generatedBy: "scripts/generate-file-fixtures.ts",
    codec,
    search,
    filter,
    ignore: {
      ...ignoreSection,
      createdFromGitignore,
      createdFromTemplate,
      syncCases,
      resetCases,
      transformNoMarkers,
    },
    tree: {
      files: treeFiles,
      dirs: treeDirs,
      modeignore: treeIgnore,
      length,
      packed,
    },
  };

  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(OUTPUT, `${JSON.stringify(fixtures, null, 2)}\n`);
  console.log(
    `[file] codec=${codecEntries.length} search=${search.queries.length} ` +
      `filter=${filter.cases.length} ignore=${ignoreSection.matchCases.length} ` +
      `tree=${length}u → ${path.relative(ROOT, OUTPUT)}`,
  );
} finally {
  for (const root of tempRoots) rmSync(root, { recursive: true, force: true });
}
