/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明初始化矩阵，拆分会降低可核对性。 */
/**
 * 生成 `data-root` 初始化器（interactive/non-interactive/import）的 Rust 对拍数据。
 *
 * `MODE_DATA_BASE_DIR` 在动态 import 初始化器之前设置，用于覆盖「隔离 base 原地补写归属」。
 * 诊断根使用真实 tmpdir，归一化为 `<tmp>/mode-startup/<pid>`。
 *
 * ```bash
 * pnpm data-root-initializer:fixtures
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
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT_DIR, "crates", "data-root", "fixtures", "initializer.json");
const SANDBOX = mkdtempSync(path.join(tmpdir(), "mode-data-root-init-"));
const ENV_BASE = path.join(SANDBOX, "env-base");
process.env.MODE_DATA_BASE_DIR = ENV_BASE;

const MANIFEST = {
  product: "dev.mode.app",
  schemaVersion: 1,
  createdBy: "desktop",
  createdAt: "2026-01-01T00:00:00.000Z",
  firstSeenVersion: "0.0.1",
};

interface Case {
  name: string;
  kind: "interactive" | "noninteractive" | "import";
  base: "isolated" | string;
  files: Record<string, string>;
  dirs: string[];
  action?: "fresh" | "migrate" | "fail";
  cancel?: boolean;
}

const CASES: Case[] = [
  { name: "interactive-absent-fresh", kind: "interactive", base: "absent", files: {}, dirs: [] },
  {
    name: "interactive-normal",
    kind: "interactive",
    base: "normal",
    files: { ".mode/.mode-root.json": JSON.stringify(MANIFEST) },
    dirs: [".mode"],
  },
  {
    name: "interactive-pending-legacy",
    kind: "interactive",
    base: "pending",
    files: { ".zcode/a.txt": "legacy" },
    dirs: [".zcode"],
  },
  {
    name: "interactive-pending-unowned",
    kind: "interactive",
    base: "unowned",
    files: { ".mode/.mode-root.json": JSON.stringify({ ...MANIFEST, product: "dev.other" }) },
    dirs: [".mode"],
  },
  {
    name: "interactive-pending-corrupt",
    kind: "interactive",
    base: "corrupt",
    files: { ".mode/.mode-root.json": "{bad" },
    dirs: [".mode"],
  },
  {
    name: "interactive-isolated-adopt",
    kind: "interactive",
    base: "isolated",
    files: { ".mode/v2/logs/a.log": "log" },
    dirs: [".mode/v2/logs"],
  },
  {
    name: "noninteractive-normal",
    kind: "noninteractive",
    base: "ni-normal",
    files: { ".mode/.mode-root.json": JSON.stringify(MANIFEST) },
    dirs: [".mode"],
  },
  { name: "noninteractive-fail", kind: "noninteractive", base: "ni-fail", files: {}, dirs: [], action: "fail" },
  {
    name: "noninteractive-fresh-legacy",
    kind: "noninteractive",
    base: "ni-fresh",
    files: { ".zcode/a.txt": "legacy" },
    dirs: [".zcode"],
    action: "fresh",
  },
  {
    name: "noninteractive-fresh-unowned",
    kind: "noninteractive",
    base: "ni-unowned",
    files: { ".mode/stray.txt": "stray" },
    dirs: [".mode"],
    action: "fresh",
  },
  {
    name: "noninteractive-migrate",
    kind: "noninteractive",
    base: "ni-migrate",
    files: { ".zcode/a.txt": "legacy" },
    dirs: [".zcode"],
    action: "migrate",
  },
  {
    name: "noninteractive-migrate-no-candidates",
    kind: "noninteractive",
    base: "ni-migrate-empty",
    files: {},
    dirs: [],
    action: "migrate",
  },
  {
    name: "noninteractive-isolated-adopt",
    kind: "noninteractive",
    base: "isolated",
    files: { ".mode/v2/logs/a.log": "log" },
    dirs: [".mode/v2/logs"],
    action: "fresh",
  },
  {
    name: "noninteractive-corrupt-fresh",
    kind: "noninteractive",
    base: "ni-corrupt",
    files: { ".mode/.mode-root.json": "{bad" },
    dirs: [".mode"],
    action: "fresh",
  },
  {
    name: "import-existing-root",
    kind: "import",
    base: "import-existing",
    files: { ".mode/old.txt": "old", ".zcode/new.txt": "new" },
    dirs: [".mode", ".zcode"],
  },
  {
    name: "import-no-root",
    kind: "import",
    base: "import-none",
    files: { ".zcode/new.txt": "new" },
    dirs: [".zcode"],
  },
  {
    name: "import-cancelled",
    kind: "import",
    base: "import-cancel",
    files: { ".mode/old.txt": "old", ".zcode/new.txt": "new" },
    dirs: [".mode", ".zcode"],
    cancel: true,
  },
];

function materialize(base: string, files: Record<string, string>, dirs: string[]): void {
  mkdirSync(base, { recursive: true });
  for (const dir of dirs) mkdirSync(path.join(base, dir), { recursive: true });
  for (const [relative, content] of Object.entries(files)) {
    const target = path.join(base, relative);
    mkdirSync(path.dirname(target), { recursive: true });
    writeFileSync(target, content.replaceAll("<root>", base));
  }
}

function normalizeString(text: string): string {
  const tmp = tmpdir();
  return text
    .replaceAll(ENV_BASE, "<root>")
    .replaceAll(ENV_BASE.replaceAll("\\", "\\\\"), "<root>")
    .replaceAll(SANDBOX, "<root>")
    .replaceAll(SANDBOX.replaceAll("\\", "\\\\"), "<root>")
    .replaceAll(tmp, "<tmp>")
    .replaceAll(tmp.replaceAll("\\", "\\\\"), "<tmp>")
    .replace(/mode-startup[\\/]\d+/gu, "mode-startup/<pid>")
    .replace(/\.mode\.(unowned|corrupt|import)-\d{8}-\d{6}(-\d+)?/gu, ".mode.$1-<ts>")
    .replace(/\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z/gu, "<ts>");
}

function normalize<T>(value: T): T {
  return JSON.parse(normalizeString(JSON.stringify(value))) as T;
}

function snapshotTree(root: string): unknown[] {
  const entries: Array<Record<string, unknown>> = [];
  const walk = (dir: string, relative: string): void => {
    if (!existsSync(dir)) return;
    for (const entry of readdirSync(dir, { withFileTypes: true }).sort((a, b) =>
      a.name < b.name ? -1 : 1,
    )) {
      const childRelative = relative ? `${relative}/${entry.name}` : entry.name;
      const target = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        entries.push({ path: normalizeString(childRelative), kind: "dir" });
        walk(target, childRelative);
      } else if (entry.isFile()) {
        entries.push({
          path: normalizeString(childRelative),
          kind: "file",
          content: normalizeString(readFileSync(target, "utf8")),
        });
      }
    }
  };
  walk(root, "");
  entries.sort((a, b) => (String(a.path) < String(b.path) ? -1 : 1));
  return entries;
}

function removeDiagnosticRoot(): void {
  rmSync(path.join(tmpdir(), "mode-startup", String(process.pid)), {
    recursive: true,
    force: true,
  });
}

async function main(): Promise<void> {
  const { initializeDataRootInteractive, initializeDataRootNonInteractive, executeDataRootImport,
    resolveDataRootActionFromEnv, resetDataRootInitializerForTest, getActiveDiagnosticRoot } =
    await import("../packages/services/src/data-root/initializer.js");
  const { discoverLegacyDataRootCandidates } = await import(
    "../packages/services/src/data-root/migration.js"
  );
  mkdirSync(ENV_BASE, { recursive: true });
  const cases: unknown[] = [];
  try {
    for (const testCase of CASES) {
      const base = testCase.base === "isolated" ? ENV_BASE : path.join(SANDBOX, testCase.base);
      rmSync(base, { recursive: true, force: true });
      materialize(base, testCase.files, testCase.dirs);
      resetDataRootInitializerForTest();
      removeDiagnosticRoot();
      let result: unknown;
      try {
        if (testCase.kind === "interactive") {
          result = initializeDataRootInteractive({ baseDir: base, createdBy: "desktop", appVersion: "0.0.1" });
        } else if (testCase.kind === "noninteractive") {
          result = await initializeDataRootNonInteractive({
            baseDir: base,
            createdBy: "desktop",
            appVersion: "0.0.1",
            action: testCase.action ?? "fresh",
          });
        } else {
          result = await executeDataRootImport({
            baseDir: base,
            candidates: discoverLegacyDataRootCandidates(base),
            createdBy: "desktop",
            appVersion: "0.0.1",
            ...(testCase.cancel ? { isCancelled: () => true } : {}),
          });
        }
      } catch (error) {
        result = { error: error instanceof Error ? error.message : String(error) };
      }
      const activeDiagnosticRoot = getActiveDiagnosticRoot();
      cases.push({
        name: testCase.name,
        kind: testCase.kind,
        base: testCase.base,
        files: testCase.files,
        dirs: testCase.dirs,
        action: testCase.action ?? null,
        cancel: testCase.cancel ?? false,
        expected: {
          result: normalize(result),
          activeDiagnosticRoot: activeDiagnosticRoot ? normalizeString(activeDiagnosticRoot) : null,
          tree: snapshotTree(base),
        },
      });
      resetDataRootInitializerForTest();
      removeDiagnosticRoot();
    }
  } finally {
    rmSync(SANDBOX, { recursive: true, force: true });
    removeDiagnosticRoot();
  }

  const fixture = {
    action: ["fresh", "MIGRATE", " fail ", "staging", "", undefined].map((value) => ({
      value: value ?? null,
      expected: resolveDataRootActionFromEnv(
        value === undefined ? {} : { MODE_DATA_ROOT_ACTION: value },
      ),
    })),
    cases,
  };
  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(OUTPUT, `${JSON.stringify(fixture, null, 2)}\n`);
  process.stdout.write(`wrote ${path.relative(ROOT_DIR, OUTPUT)} (${CASES.length} cases)\n`);
}

await main();
