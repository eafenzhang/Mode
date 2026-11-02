/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明数据根状态矩阵，拆分会降低可核对性。 */
/**
 * 生成 `data-root`（归属文件/状态/备份）的 Rust 对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/shared/src/data-root.ts`
 * - `packages/services/src/data-root/ownership.ts`
 *
 * 每条用例独立沙箱目录；路径归一化为 `<root>`，备份时间戳归一化为 `<ts>`。
 *
 * ```bash
 * pnpm data-root:fixtures
 * cargo test -p data-root
 * ```
 */
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  DATA_ROOT_PRODUCT_ID,
  isAcceptedDataRootProduct,
  parseDataRootManifest,
} from "../packages/shared/src/data-root.js";
import {
  forfeitConflictingDataRoot,
  forfeitDataRootByLabel,
  readDataRootStatus,
  readLegacyDataBaseDirFromSettings,
  writeDataRootManifestIntoRoot,
} from "../packages/services/src/data-root/ownership.js";
import {
  cleanupStaleMigrationStaging,
  collectLegacyCandidateStats,
  discoverLegacyDataRootCandidates,
  executeDataRootCopyMigration,
} from "../packages/services/src/data-root/migration.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT_DIR, "crates", "data-root", "fixtures", "data-root.json");

const MANIFEST = {
  product: DATA_ROOT_PRODUCT_ID,
  schemaVersion: 1,
  createdBy: "desktop",
  createdAt: "2026-01-01T00:00:00.000Z",
  firstSeenVersion: "0.0.1",
};

const MANIFEST_CASES = [
  {
    name: "full",
    raw: JSON.stringify({
      ...MANIFEST,
      migration: { from: "/old", at: "2026-01-01", mode: "copy" },
    }),
  },
  { name: "minimal", raw: JSON.stringify(MANIFEST) },
  { name: "legacy-product", raw: JSON.stringify({ ...MANIFEST, product: "dev.zcodium.app" }) },
  { name: "extra-keys", raw: JSON.stringify({ ...MANIFEST, ignored: { any: true } }) },
  { name: "not-json", raw: "not json" },
  { name: "array", raw: "[]" },
  { name: "empty-product", raw: JSON.stringify({ ...MANIFEST, product: "   " }) },
  { name: "fractional-schema", raw: JSON.stringify({ ...MANIFEST, schemaVersion: 1.5 }) },
  { name: "string-schema", raw: JSON.stringify({ ...MANIFEST, schemaVersion: "1" }) },
  { name: "bad-created-by", raw: JSON.stringify({ ...MANIFEST, createdBy: "robot" }) },
  { name: "empty-created-at", raw: JSON.stringify({ ...MANIFEST, createdAt: "" }) },
  { name: "migration-null", raw: JSON.stringify({ ...MANIFEST, migration: null }) },
  {
    name: "migration-missing-from",
    raw: JSON.stringify({ ...MANIFEST, migration: { at: "2026-01-01", mode: "copy" } }),
  },
  {
    name: "migration-bad-mode",
    raw: JSON.stringify({
      ...MANIFEST,
      migration: { from: "/old", at: "2026-01-01", mode: "move" },
    }),
  },
];

interface StatusCase {
  name: string;
  files: Record<string, string>;
  dirs: string[];
}

const STATUS_CASES: StatusCase[] = [
  { name: "absent", files: {}, dirs: [] },
  { name: "unowned-missing", files: {}, dirs: [".mode"] },
  {
    name: "normal",
    files: { ".mode/.mode-root.json": JSON.stringify(MANIFEST) },
    dirs: [".mode"],
  },
  {
    name: "normal-legacy-filename",
    files: { ".mode/.zcodium-root.json": JSON.stringify(MANIFEST) },
    dirs: [".mode"],
  },
  {
    name: "unowned-product",
    files: { ".mode/.mode-root.json": JSON.stringify({ ...MANIFEST, product: "dev.other" }) },
    dirs: [".mode"],
  },
  {
    name: "corrupt-json",
    files: { ".mode/.mode-root.json": "{not json" },
    dirs: [".mode"],
  },
  {
    name: "corrupt-schema",
    files: { ".mode/.mode-root.json": JSON.stringify({ ...MANIFEST, schemaVersion: 2 }) },
    dirs: [".mode"],
  },
];

const LEGACY_SETTINGS_CASES = [
  {
    name: "trimmed",
    content: JSON.stringify({ dataBaseDir: " /custom/base " }),
    expected: "/custom/base",
  },
  { name: "missing-key", content: JSON.stringify({}), expected: null },
  { name: "empty", content: JSON.stringify({ dataBaseDir: "   " }), expected: null },
  { name: "not-object", content: "[]", expected: null },
  { name: "invalid-json", content: "not json", expected: null },
  { name: "missing-file", content: null, expected: null },
];

interface ForfeitCase {
  name: string;
  call: "label" | "conflict";
  label: string;
  setup: boolean;
}

const FORFEIT_CASES: ForfeitCase[] = [
  { name: "label-unowned", call: "label", label: "unowned", setup: true },
  { name: "label-corrupt", call: "label", label: "corrupt", setup: true },
  { name: "label-missing-root", call: "label", label: "unowned", setup: false },
  { name: "conflict-unowned", call: "conflict", label: "unowned", setup: true },
  { name: "conflict-corrupt", call: "conflict", label: "corrupt", setup: true },
];

function materialize(base: string, files: Record<string, string>, dirs: string[]): void {
  mkdirSync(base, { recursive: true });
  for (const dir of dirs) mkdirSync(path.join(base, dir), { recursive: true });
  for (const [relative, content] of Object.entries(files)) {
    const target = path.join(base, relative);
    mkdirSync(path.dirname(target), { recursive: true });
    writeFileSync(
      target,
      content
        .replaceAll("<root>", base)
        .replaceAll("<rootForward>", base.replaceAll("\\", "/")),
    );
  }
}

function normalizeText(text: string, root: string): string {
  return text
    .replaceAll(root, "<root>")
    .replaceAll(root.replaceAll("\\", "\\\\"), "<root>")
    .replaceAll(root.replaceAll("\\", "/"), "<root>")
    .replace(/\.mode\.(unowned|corrupt)-\d{8}-\d{6}(-\d+)?/gu, ".mode.$1-<ts>");
}

function normalizeSnapshotText(text: string, root: string): string {
  return normalizeText(text, root).replace(
    /\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z/gu,
    "<ts>",
  );
}

function snapshotTree(root: string): unknown[] {
  const entries: Array<Record<string, unknown>> = [];
  const walk = (dir: string, relative: string): void => {
    for (const entry of readdirSync(dir, { withFileTypes: true }).sort((a, b) =>
      a.name.localeCompare(b.name),
    )) {
      const childRelative = relative ? `${relative}/${entry.name}` : entry.name;
      const target = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        entries.push({ path: normalizeSnapshotText(childRelative, root), kind: "dir" });
        walk(target, childRelative);
      } else if (entry.isFile()) {
        entries.push({
          path: normalizeSnapshotText(childRelative, root),
          kind: "file",
          content: normalizeSnapshotText(readFileSync(target, "utf8"), root),
        });
      }
    }
  };
  walk(root, "");
  entries.sort((a, b) => (String(a.path) < String(b.path) ? -1 : 1));
  return entries;
}

function normalize<T>(value: T, root: string): T {
  return JSON.parse(normalizeText(JSON.stringify(value), root)) as T;
}

async function main(): Promise<void> {
  const sandbox = mkdtempSync(path.join(tmpdir(), "mode-data-root-fixtures-"));
  const status: unknown[] = [];
  const write: unknown[] = [];
  const forfeit: unknown[] = [];
  const legacySettings: unknown[] = [];
  try {
    for (const [index, testCase] of STATUS_CASES.entries()) {
      const base = path.join(sandbox, `status-${index}`);
      materialize(base, testCase.files, testCase.dirs);
      status.push({
        name: testCase.name,
        files: testCase.files,
        dirs: testCase.dirs,
        expected: JSON.parse(normalizeText(JSON.stringify(readDataRootStatus(base)), base)),
      });
    }

    const writeBase = path.join(sandbox, "write");
    const writeManifest = {
      ...MANIFEST,
      migration: { from: "/old", at: "2026-01-01", mode: "import" },
    };
    const writtenPath = writeDataRootManifestIntoRoot(
      path.join(writeBase, ".mode"),
      writeManifest as never,
    );
    write.push({
      manifest: writeManifest,
      expected: {
        path: normalizeText(writtenPath, writeBase),
        content: normalizeText(
          readFileSync(path.join(writeBase, ".mode", ".mode-root.json"), "utf8"),
          writeBase,
        ),
      },
    });

    for (const [index, testCase] of FORFEIT_CASES.entries()) {
      const base = path.join(sandbox, `forfeit-${index}`);
      mkdirSync(base, { recursive: true });
      if (testCase.setup) mkdirSync(path.join(base, ".mode"), { recursive: true });
      const backup =
        testCase.call === "label"
          ? forfeitDataRootByLabel(base, testCase.label)
          : testCase.call === "conflict"
            ? forfeitConflictingDataRoot(base, {
                kind: testCase.label,
                reason: "manifest-missing",
                rootDir: path.join(base, ".mode"),
              } as never)
            : null;
      forfeit.push({
        name: testCase.name,
        call: testCase.call,
        label: testCase.label,
        setup: testCase.setup,
        expected: {
          backup: backup === null ? null : normalizeText(backup, base),
          rootExistsAfter: existsSync(path.join(base, ".mode")),
        },
      });
    }

    for (const [index, testCase] of LEGACY_SETTINGS_CASES.entries()) {
      const legacyRoot = path.join(sandbox, `legacy-${index}`);
      mkdirSync(path.join(legacyRoot, "v2"), { recursive: true });
      if (testCase.content !== null) {
        writeFileSync(path.join(legacyRoot, "v2", "setting.json"), testCase.content);
      }
      legacySettings.push({
        name: testCase.name,
        content: testCase.content,
        expected: readLegacyDataBaseDirFromSettings(legacyRoot),
      });
    }
  } finally {
    rmSync(sandbox, { recursive: true, force: true });
  }

  const discoveryCases: unknown[] = [];
  const STATS_CASES = [
    { name: "empty", files: {} },
    { name: "nested", files: { "a.txt": "12345", "dir/b.txt": "1234567890", "dir/sub/c.bin": "x" } },
  ];
  const statsCases: unknown[] = [];
  for (const [index, testCase] of STATS_CASES.entries()) {
    const root = path.join(sandbox, `stats-${index}`);
    materialize(root, testCase.files, []);
    const stats = await collectLegacyCandidateStats(root);
    statsCases.push({
      name: testCase.name,
      files: testCase.files,
      expected: {
        sizeBytes: stats.sizeBytes,
        itemCount: stats.itemCount,
        modifiedAtPresent: stats.modifiedAt !== null,
      },
    });
  }
  const DISCOVERY_CASES = [
    { name: "primary-zcode", files: {}, dirs: [".zcode"] },
    { name: "both-roots", files: {}, dirs: [".zcodium", ".zcode"] },
    { name: "none", files: {}, dirs: [] },
    {
      name: "custom-base",
      files: {
        ".zcodium/v2/setting.json": JSON.stringify({ dataBaseDir: "<rootForward>/custom" }),
      },
      dirs: [".zcodium", "custom/.zcode"],
    },
    {
      name: "home-expansion",
      files: {
        ".zcodium/v2/setting.json": JSON.stringify({ dataBaseDir: "~/custombase" }),
      },
      dirs: [".zcodium", "home/custombase/.zcode"],
    },
    {
      name: "duplicate-custom",
      files: {
        ".zcodium/v2/setting.json": JSON.stringify({ dataBaseDir: "<rootForward>" }),
      },
      dirs: [".zcodium"],
    },
    {
      name: "custom-without-legacy",
      files: {
        ".zcodium/v2/setting.json": JSON.stringify({ dataBaseDir: "<rootForward>/custom" }),
      },
      dirs: [".zcodium", "custom"],
    },
  ];
  for (const [index, testCase] of DISCOVERY_CASES.entries()) {
    const root = path.join(sandbox, `discovery-${index}`);
    const home = path.join(root, "home");
    materialize(root, testCase.files, testCase.dirs);
    mkdirSync(home, { recursive: true });
    const previousHome = process.env.HOME;
    const previousUserProfile = process.env.USERPROFILE;
    process.env.HOME = home;
    process.env.USERPROFILE = home;
    try {
      const candidates = discoverLegacyDataRootCandidates(root);
      discoveryCases.push({
        name: testCase.name,
        files: testCase.files,
        dirs: testCase.dirs,
        expected: normalize(
          candidates.map((candidate) => ({
            baseDir: candidate.baseDir,
            legacyRoot: candidate.legacyRoot,
            isPrimaryBase: candidate.isPrimaryBase,
          })),
          root,
        ),
      });
    } finally {
      if (previousHome === undefined) delete process.env.HOME;
      else process.env.HOME = previousHome;
      if (previousUserProfile === undefined) delete process.env.USERPROFILE;
      else process.env.USERPROFILE = previousUserProfile;
    }
  }

  const cleanupCases: unknown[] = [];
  const CLEANUP_CASES = [
    {
      name: "mixed",
      entries: [
        { name: ".mode.migrating-old", ageMs: 2 * 3600_000, directory: true },
        { name: ".mode.migrating-fresh", ageMs: 0, directory: true },
        { name: ".mode.migrating-file", ageMs: 2 * 3600_000, directory: false },
        { name: "other", ageMs: 2 * 3600_000, directory: true },
      ],
    },
    { name: "none", entries: [] },
  ];
  for (const [index, testCase] of CLEANUP_CASES.entries()) {
    const root = path.join(sandbox, `cleanup-${index}`);
    mkdirSync(root, { recursive: true });
    const now = Date.now();
    for (const entry of testCase.entries) {
      const target = path.join(root, entry.name);
      if (entry.directory) mkdirSync(target, { recursive: true });
      else writeFileSync(target, "");
      const stamp = new Date(now - entry.ageMs);
      utimesSync(target, stamp, stamp);
    }
    const before = readdirSync(root);
    cleanupStaleMigrationStaging(root);
    await new Promise((resolve) => setTimeout(resolve, 20));
    const after = readdirSync(root);
    cleanupCases.push({
      name: testCase.name,
      entries: testCase.entries,
      expected: {
        removed: normalize(
          before.filter((name) => !after.includes(name)).map((name) => path.join(root, name)),
          root,
        ),
        surviving: after.sort(),
      },
    });
  }

  const COPY_CASES = [
    {
      name: "fresh",
      files: { ".zcode/a.txt": "hello" },
      dirs: [".zcode"],
      mode: "copy",
      cancel: false,
    },
    {
      name: "import-mode",
      files: { ".zcodium/b.txt": "1234567890" },
      dirs: [".zcodium"],
      mode: "import",
      cancel: false,
    },
    {
      name: "conflict-unowned",
      files: { ".mode/stray.txt": "stray", ".zcode/a.txt": "abc" },
      dirs: [".mode", ".zcode"],
      mode: "copy",
      cancel: false,
    },
    {
      name: "conflict-normal",
      files: {
        ".mode/.mode-root.json": JSON.stringify({
          ...MANIFEST,
          createdAt: "2026-01-01T00:00:00.000Z",
        }),
        ".mode/existing.txt": "keep",
        ".zcode/a.txt": "abc",
      },
      dirs: [".mode", ".zcode"],
      mode: "copy",
      cancel: false,
    },
    {
      name: "cancelled",
      files: { ".zcode/a.txt": "abc" },
      dirs: [".zcode"],
      mode: "copy",
      cancel: true,
    },
    { name: "no-candidates", files: {}, dirs: [], mode: "copy", cancel: false },
    {
      name: "multi-candidate",
      files: {
        ".zcodium/primary.txt": "p",
        ".zcodium/v2/setting.json": JSON.stringify({ dataBaseDir: "<rootForward>/custom" }),
        "custom/.zcode/custom.txt": "c",
      },
      dirs: [".zcodium", "custom/.zcode"],
      mode: "copy",
      cancel: false,
      compareProgress: false,
    },
    {
      name: "transient-files",
      files: {
        ".zcode/keep.txt": "k",
        ".zcode/.DS_Store": "x",
        ".zcode/a.lock": "x",
        ".zcode/setting.json.abc.tmp": "x",
        ".zcode/setting.json.lock": "x",
      },
      dirs: [".zcode"],
      mode: "copy",
      cancel: false,
    },
    {
      name: "nested",
      files: { ".zcode/a.txt": "1", ".zcode/dir/sub/b.txt": "22" },
      dirs: [".zcode", ".zcode/empty"],
      mode: "copy",
      cancel: false,
      compareProgress: false,
    },
  ];
  const copyMigration: unknown[] = [];
  for (const [index, testCase] of COPY_CASES.entries()) {
    const root = path.join(sandbox, `copy-${index}`);
    const home = path.join(root, "home");
    materialize(root, testCase.files, testCase.dirs);
    mkdirSync(home, { recursive: true });
    const previousHome = process.env.HOME;
    const previousUserProfile = process.env.USERPROFILE;
    process.env.HOME = home;
    process.env.USERPROFILE = home;
    const progress: unknown[] = [];
    try {
      const candidates = discoverLegacyDataRootCandidates(root);
      const result = await executeDataRootCopyMigration({
        candidates,
        createdBy: "desktop",
        appVersion: "0.0.1",
        mode: testCase.mode as never,
        onProgress: (event) => {
          progress.push(
            normalize(
              {
                phase: event.phase,
                copiedBytes: event.copiedBytes,
                totalBytes: event.totalBytes,
                currentBaseDir: event.currentBaseDir ?? null,
              },
              root,
            ),
          );
        },
        isCancelled: () => testCase.cancel,
      });
      copyMigration.push({
        name: testCase.name,
        files: testCase.files,
        dirs: testCase.dirs,
        mode: testCase.mode,
        cancel: testCase.cancel,
        compareProgress: testCase.compareProgress ?? progress.length <= 3,
        expected: {
          result: normalize(result, root),
          tree: snapshotTree(root),
          progress: testCase.compareProgress === false ? null : progress,
        },
      });
    } finally {
      if (previousHome === undefined) delete process.env.HOME;
      else process.env.HOME = previousHome;
      if (previousUserProfile === undefined) delete process.env.USERPROFILE;
      else process.env.USERPROFILE = previousUserProfile;
    }
  }

  const fixture = {
    copyMigration,
    discovery: discoveryCases,
    stats: statsCases,
    cleanup: cleanupCases,
    manifestParse: MANIFEST_CASES.map((testCase) => ({
      name: testCase.name,
      raw: testCase.raw,
      expected: parseDataRootManifest(testCase.raw),
    })),
    product: [
      { product: DATA_ROOT_PRODUCT_ID, expected: true },
      { product: "dev.zcodium.app", expected: true },
      { product: "dev.other", expected: false },
    ].map((testCase) => ({
      ...testCase,
      actual: isAcceptedDataRootProduct(testCase.product),
    })),
    status,
    write,
    forfeit,
    legacySettings,
  };
  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(OUTPUT, `${JSON.stringify(fixture, null, 2)}\n`);
  process.stdout.write(`wrote ${path.relative(ROOT_DIR, OUTPUT)}\n`);
}

await main();
