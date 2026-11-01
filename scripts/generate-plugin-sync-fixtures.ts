/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明 plugin-sync 用例，拆分会降低可核对性。 */
/**
 * 生成 `plugin-sync-service` 与 TS 真源的对拍数据（真实 home 目录）。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/plugin-sync/pluginSyncService.ts`
 * - `packages/services/src/plugin-sync/pluginSyncPath.ts`
 *
 * 只覆盖本 crate 已迁移的三个方法（候选扫描 / 远端判重 / 写权限）；
 * 归档导出/导入在后续切片补。`HOME`/`USERPROFILE` 在调用服务前指向沙盒。
 *
 * ```bash
 * pnpm plugin-sync:fixtures
 * cargo test -p plugin-sync-service
 * ```
 */
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, statSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(
  ROOT_DIR,
  "crates",
  "plugin-sync-service",
  "fixtures",
  "plugin-sync.json",
);
const SANDBOX = realpathSync.native(mkdtempSync(path.join(tmpdir(), "zc-plugin-sync-fixtures-")));

interface FileSpec {
  path: string;
  content: string;
}

interface FixtureCase {
  name: string;
  setup: {
    files: FileSpec[];
    dirs: string[];
    dirsMissing?: string[];
    skipConfig?: boolean;
    invalidConfig?: boolean;
  };
  ops: Op[];
}

type Op =
  | { kind: "candidates" }
  | { kind: "statuses"; plugins: Array<{ pluginId: string; directoryName: string }> }
  | { kind: "writeAccess" }
  | { kind: "export"; pluginNames: string[] }
  | { kind: "import"; from: number }
  | { kind: "importOverwrite"; from: number }
  | { kind: "setDirs"; dirs: string[] };

function homeDir(): string {
  return path.join(SANDBOX, "home");
}

function materialize(files: FileSpec[]): void {
  for (const file of files) {
    const absolute = path.join(SANDBOX, ...file.path.split("/"));
    mkdirSync(path.dirname(absolute), { recursive: true });
    writeFileSync(absolute, file.content);
  }
}

function pluginFiles(root: string, manifestDir: string, manifest: object): FileSpec[] {
  return [
    { path: `${root}/${manifestDir}/plugin.json`, content: JSON.stringify(manifest, null, 2) },
  ];
}

function writeConfig(dirs: string[]): void {
  const configPath = path.join(homeDir(), ".mode", "cli", "config.json");
  mkdirSync(path.dirname(configPath), { recursive: true });
  const absolute = dirs.map((dir) => path.join(SANDBOX, ...dir.split("/")));
  writeFileSync(
    configPath,
    `${JSON.stringify(
      {
        plugins: {
          dirs: absolute,
          enabledPlugins: {
            "plugin a@inline": false,
            "pluginb@inline": true,
            "not-a-bool@inline": "yes",
          },
        },
      },
      null,
      2,
    )}
`,
  );
}

function fixMtimes(root: string): void {
  const fixed = new Date("2024-01-01T00:00:00Z");
  const walk = (dir: string): void => {
    for (const name of readdirSync(dir)) {
      const absolute = path.join(dir, name);
      if (statSync(absolute).isDirectory()) walk(absolute);
      utimesSync(absolute, fixed, fixed);
    }
    utimesSync(dir, fixed, fixed);
  };
  walk(root);
}

function setupCase(testCase: FixtureCase): void {
  rmSync(SANDBOX, { recursive: true, force: true });
  mkdirSync(homeDir(), { recursive: true });
  materialize(testCase.setup.files);
  if (testCase.setup.skipConfig) {
    fixMtimes(SANDBOX);
    return;
  }
  const configPath = path.join(homeDir(), ".mode", "cli", "config.json");
  mkdirSync(path.dirname(configPath), { recursive: true });
  if (testCase.setup.invalidConfig) {
    writeFileSync(configPath, "{not json");
    fixMtimes(SANDBOX);
    return;
  }
  const dirs = testCase.setup.dirs.map((dir) => path.join(SANDBOX, ...dir.split("/")));
  writeFileSync(
    configPath,
    `${JSON.stringify(
      {
        plugins: {
          dirs,
          enabledPlugins: {
            "plugin a@inline": false,
            "pluginb@inline": true,
            "not-a-bool@inline": "yes",
          },
        },
      },
      null,
      2,
    )}\n`,
  );
  fixMtimes(SANDBOX);
}

function listFiles(root: string): Array<{ path: string; content: string }> {
  if (!existsSync(root)) return [];
  const entries: Array<{ path: string; content: string }> = [];
  const walk = (dir: string, prefix: string): void => {
    for (const name of readdirSync(dir).sort()) {
      const absolute = path.join(dir, name);
      const relative = prefix ? `${prefix}/${name}` : name;
      if (statSync(absolute).isDirectory()) {
        walk(absolute, relative);
        continue;
      }
      entries.push({ path: relative, content: readFileSync(absolute, "utf-8") });
    }
  };
  walk(root, "");
  return entries;
}

function normalizeText(text: string): string {
  const native = SANDBOX;
  const forward = native.split(path.sep).join("/");
  const escaped = native.split("\\").join("\\\\");
  let replaced = text.replaceAll(escaped, "<root>");
  replaced = replaced.replaceAll(native, "<root>");
  replaced = replaced.replaceAll(forward, "<root>");
  if (path.sep === "\\") {
    replaced = replaced.split("\\\\").join("/").split("\\").join("/");
  }
  return replaced;
}

const SHA256 = /^[0-9a-f]{64}$/u;

function normalizeValue(value: unknown, key?: string): unknown {
  if (key === "archive") return "<archive>";
  if (key === "archiveBytes") return "<bytes>";
  if (typeof value === "string") {
    let replaced = normalizeText(value);
    if (SHA256.test(replaced)) return "<sha256>";
    // 目录名去重后缀基于绝对路径，跨沙盒不稳定。
    replaced = replaced.replace(/-[0-9a-f]{8}$/u, "-<hash8>");
    return replaced;
  }
  if (Array.isArray(value)) return value.map((item) => normalizeValue(item));
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [childKey, child] of Object.entries(value as Record<string, unknown>)) {
      out[childKey] = normalizeValue(child, childKey);
    }
    return out;
  }
  return value;
}

const cases: FixtureCase[] = [
  {
    name: "candidates-status-write-access",
    setup: {
      dirs: [
        "plugins/plugin-a",
        "plugins/plugin-b",
        "plugins/plugin-a",
        "plugins/dup-a/shared",
        "plugins/dup-b/shared",
        "plugins/broken",
        "plugins/no-manifest",
      ],
      files: [
        ...pluginFiles("plugins/plugin-a", ".mode-plugin", {
          name: "Plugin A",
          description: " A desc ",
          version: " 1.0.0 ",
        }),
        { path: "plugins/plugin-a/skills/demo/SKILL.md", content: "# demo\n" },
        { path: "plugins/plugin-a/commands/demo.md", content: "# demo\n" },
        { path: "plugins/plugin-a/.mcp.json", content: "{}\n" },
        ...pluginFiles("plugins/plugin-b", ".claude-plugin", { name: "Plugin B" }),
        { path: "plugins/plugin-b/hooks/hooks.json", content: "{}\n" },
        ...pluginFiles("plugins/dup-a/shared", ".mode-plugin", { name: "Shared A" }),
        ...pluginFiles("plugins/dup-b/shared", ".mode-plugin", { name: "Shared B" }),
        { path: "plugins/broken/.zcode-plugin/plugin.json", content: "{not json" },
        { path: "plugins/no-manifest/readme.md", content: "x\n" },
        { path: "home/.mode/plugins/existing-dir/file.txt", content: "x\n" },
      ],
    },
    ops: [
      { kind: "candidates" },
      {
        kind: "statuses",
        plugins: [
          { pluginId: "custom@inline", directoryName: "existing-dir" },
          { pluginId: "plugin b@INLINE", directoryName: "new-b" },
          { pluginId: "missing@inline", directoryName: "missing-dir" },
        ],
      },
      { kind: "writeAccess" },
      { kind: "export", pluginNames: ["Plugin A", "Plugin B"] },
      { kind: "setDirs", dirs: ["plugins/plugin-a"] },
      { kind: "import", from: 0 },
      { kind: "import", from: 0 },
      { kind: "importOverwrite", from: 0 },
      { kind: "export", pluginNames: ["Missing Plugin"] },
    ],
  },
  {
    name: "empty-home",
    setup: { files: [], dirs: [], skipConfig: true },
    ops: [
      { kind: "candidates" },
      { kind: "statuses", plugins: [{ pluginId: "x@inline", directoryName: "x" }] },
      { kind: "writeAccess" },
    ],
  },
  {
    name: "invalid-config",
    setup: { files: [], dirs: [], invalidConfig: true },
    ops: [
      { kind: "candidates" },
      { kind: "statuses", plugins: [{ pluginId: "x@inline", directoryName: "x" }] },
      { kind: "writeAccess" },
    ],
  },
];

async function main(): Promise<void> {
  process.env.HOME = homeDir();
  process.env.USERPROFILE = homeDir();

  const { createPluginSyncService } = await import(
    "../packages/services/src/plugin-sync/pluginSyncService.js"
  );

  const results = [];
  for (const testCase of cases) {
    setupCase(testCase);
    const service = createPluginSyncService();
    const expected: unknown[] = [];
    const archives: Uint8Array[] = [];
    for (const op of testCase.ops) {
      try {
        if (op.kind === "candidates") {
          expected.push(normalizeValue(await service.listLocalUserPluginCandidates()));
        } else if (op.kind === "statuses") {
          expected.push(
            normalizeValue(await service.listRemoteUserPluginStatuses({ plugins: op.plugins })),
          );
        } else if (op.kind === "writeAccess") {
          expected.push(normalizeValue(await service.checkRemoteUserPluginWriteAccess()));
        } else if (op.kind === "export") {
          const candidates = await service.listLocalUserPluginCandidates();
          const ids = op.pluginNames.map((name) => {
            const candidate = candidates.candidates.find((item) => item.name === name);
            if (!candidate) throw new Error(`fixture candidate not found: ${name}`);
            return candidate.id;
          });
          // 归档 metadata 条目默认用 Date.now() 作 mtime；固定时钟保证 fixture 幂等。
          const realNow = Date.now;
          Date.now = () => 1704067200000;
          let result;
          try {
            result = await service.exportPluginsArchive({ pluginIds: ids });
          } finally {
            Date.now = realNow;
          }
          archives.push(result.archive);
          expected.push(normalizeValue(result));
        } else if (op.kind === "import") {
          expected.push(
            normalizeValue(await service.importPluginsArchive({ archive: archives[op.from]! })),
          );
        } else if (op.kind === "importOverwrite") {
          await service.importPluginsArchive({
            archive: archives[op.from]!,
            overwrite: true as never,
          });
          expected.push({ ok: true });
        } else if (op.kind === "setDirs") {
          writeConfig(op.dirs);
          expected.push({ ok: true });
        }
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        // Node 的 JSON.parse 文案随版本变化；两端统一归一为 <json-error>。
        expected.push(
          normalizeValue({
            error: /JSON|json object/u.test(message) ? "<json-error>" : message,
          }),
        );
      }
    }
    const pluginRoot = path.join(homeDir(), ".mode", "plugins");
    const configPath = path.join(homeDir(), ".mode", "cli", "config.json");
    let config: unknown = null;
    if (existsSync(configPath)) {
      try {
        config = normalizeValue(JSON.parse(readFileSync(configPath, "utf-8")));
      } catch {
        config = { error: "<json-error>" };
      }
    }
    results.push({
      name: testCase.name,
      setup: testCase.setup,
      ops: testCase.ops,
      expected,
      tree: listFiles(pluginRoot),
      config,
      archiveBase64: archives[0] ? Buffer.from(archives[0]).toString("base64") : null,
    });
  }

  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(OUTPUT, `${JSON.stringify({ cases: results }, null, 2)}\n`);
  process.stdout.write(`wrote ${path.relative(ROOT_DIR, OUTPUT)} (${results.length} cases)\n`);
  rmSync(SANDBOX, { recursive: true, force: true });
}

await main();
