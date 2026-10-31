/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明 settings-sync 用例，拆分会降低可核对性。 */
/**
 * 生成 `settings-sync-service` 与 TS 真源的对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/settings-sync/settingsSyncService.ts`
 *
 * ```bash
 * pnpm settings-sync:fixtures
 * cargo test -p settings-sync-service
 * ```
 */
import {
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(
  ROOT_DIR,
  "crates",
  "settings-sync-service",
  "fixtures",
  "settings-sync.json",
);

const SANDBOX = realpathSync.native(mkdtempSync(path.join(tmpdir(), "zc-settings-sync-fixtures-")));
const HOME = path.join(SANDBOX, "home");
const WORKSPACE = path.join(SANDBOX, "ws");

interface FileSpec {
  path: string;
  content: string;
}

interface FixtureOp {
  kind:
    | "detect"
    | "importSelected"
    | "agentsFileStatus"
    | "copyAgentsFile"
    | "firstRunState"
    | "markFirstRun";
  categories?: string[];
  intent?: "firstRun" | "manualImport";
  useWorkspace?: boolean;
  selections?: unknown[];
  overwrite?: boolean;
}

interface FixtureCase {
  name: string;
  files: FileSpec[];
  ops: FixtureOp[];
  expected: unknown[];
  tree: FileSpec[];
}

interface SettingsSyncServiceLike {
  detect(params: {
    workspacePath?: string;
    categories?: string[];
    intent?: "firstRun" | "manualImport";
  }): Promise<unknown>;
  importSelected(params: { workspacePath?: string; selections: unknown[] }): Promise<unknown>;
  getClaudeAgentsFileMigrationStatus(params: object): Promise<unknown>;
  copyClaudeAgentsFileToModeAgentsFile(params: { overwrite?: boolean }): Promise<unknown>;
  getFirstRunPromptState(): Promise<unknown>;
  markFirstRunPromptHandled(): Promise<void>;
}

function forwardRoot(): string {
  return SANDBOX.split(path.sep).join("/");
}

function materialize(root: string, files: FileSpec[]): void {
  for (const file of files) {
    const target = path.join(root, file.path);
    mkdirSync(path.dirname(target), { recursive: true });
    writeFileSync(target, file.content.replaceAll("<root>", forwardRoot()));
  }
}

function normalizeText(root: string, text: string): string {
  const native = root;
  const forward = root.split(path.sep).join("/");
  const escaped = native.split("\\").join("\\\\");
  let replaced = text.replaceAll(escaped, "<root>");
  replaced = replaced.replaceAll(native, "<root>");
  replaced = replaced.replaceAll(forward, "<root>");
  if (path.sep === "\\") {
    replaced = replaced.split("\\\\").join("/").split("\\").join("/");
  }
  return replaced;
}

function normalizeValue(root: string, value: unknown): unknown {
  if (typeof value === "string") return normalizeText(root, value);
  if (Array.isArray(value)) return value.map((item) => normalizeValue(root, item));
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [key, child] of Object.entries(value as Record<string, unknown>)) {
      out[key] = normalizeValue(root, child);
    }
    return out;
  }
  return value;
}

function sortDeep(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortDeep);
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const key of Object.keys(value as Record<string, unknown>).sort()) {
      out[key] = sortDeep((value as Record<string, unknown>)[key]);
    }
    return out;
  }
  return value;
}

function normalizeJsonFile(root: string, content: string): string {
  return normalizeText(root, JSON.stringify(sortDeep(JSON.parse(content)), null, 2));
}

function captureTree(root: string): FileSpec[] {
  const files: FileSpec[] = [];
  const walk = (directory: string, relative: string): void => {
    const entries = readdirSync(directory, { withFileTypes: true }).sort((left, right) =>
      left.name.localeCompare(right.name),
    );
    for (const entry of entries) {
      const absolute = path.join(directory, entry.name);
      const childRelative = relative ? `${relative}/${entry.name}` : entry.name;
      if (entry.isDirectory()) {
        walk(absolute, childRelative);
      } else if (entry.isFile()) {
        const raw = readFileSync(absolute, "utf8");
        files.push({
          path: childRelative,
          content: entry.name.endsWith(".json")
            ? normalizeJsonFile(root, raw)
            : normalizeText(root, raw),
        });
      }
    }
  };
  walk(root, "");
  return files.sort((left, right) => left.path.localeCompare(right.path));
}

async function runOp(service: SettingsSyncServiceLike, op: FixtureOp): Promise<unknown> {
  try {
    switch (op.kind) {
      case "detect":
        return await service.detect({
          ...(op.useWorkspace ? { workspacePath: WORKSPACE } : {}),
          categories: op.categories,
          intent: op.intent,
        });
      case "importSelected":
        return await service.importSelected({
          ...(op.useWorkspace ? { workspacePath: WORKSPACE } : {}),
          selections: (op.selections ?? []).map((selection) => {
            const entry = selection as Record<string, unknown>;
            const resolved: Record<string, unknown> = { ...entry };
            for (const key of ["skillPaths", "commandPaths", "pluginPaths", "mcpServerPaths"]) {
              const value = entry[key];
              if (Array.isArray(value)) {
                resolved[key] = value.map((item) =>
                  typeof item === "string"
                    ? path.normalize(item.replaceAll("<root>", SANDBOX))
                    : item,
                );
              }
            }
            return resolved;
          }),
        });
      case "agentsFileStatus":
        return await service.getClaudeAgentsFileMigrationStatus({});
      case "copyAgentsFile":
        return await service.copyClaudeAgentsFileToModeAgentsFile({
          overwrite: op.overwrite,
        });
      case "firstRunState":
        return await service.getFirstRunPromptState();
      case "markFirstRun":
        await service.markFirstRunPromptHandled();
        return { ok: true };
      default:
        throw new Error(`unknown op: ${String((op as { kind?: string }).kind)}`);
    }
  } catch (error) {
    return { error: error instanceof Error ? error.message : String(error) };
  }
}

function createFakeSettingService(): {
  get(): Promise<Record<string, unknown>>;
  update(patch: Record<string, unknown>): Promise<Record<string, unknown>>;
} {
  let settings: Record<string, unknown> = {};
  return {
    async get() {
      return { ...settings };
    },
    async update(patch) {
      settings = { ...settings, ...patch };
      return { ...settings };
    },
  };
}

async function runCase(
  createService: (dependencies: { settingService: unknown }) => SettingsSyncServiceLike,
  testCase: Omit<FixtureCase, "expected" | "tree">,
): Promise<FixtureCase> {
  rmSync(SANDBOX, { recursive: true, force: true });
  mkdirSync(SANDBOX, { recursive: true });
  materialize(SANDBOX, testCase.files);

  const service = createService({ settingService: createFakeSettingService() });
  const expected: unknown[] = [];
  for (const op of testCase.ops) {
    expected.push(normalizeValue(SANDBOX, await runOp(service, op)));
  }
  return {
    name: testCase.name,
    files: testCase.files,
    ops: testCase.ops,
    expected,
    tree: captureTree(SANDBOX),
  };
}

function skillFile(name: string, description: string, version?: string): string {
  return `---\nname: ${name}\ndescription: ${description}\n${version ? `version: ${version}\n` : ""}---\n${name} body\n`;
}

const FILES: FileSpec[] = [
  {
    path: "home/.claude/skills/alpha/SKILL.md",
    content: skillFile("alpha", "claude alpha", "1.0.0"),
  },
  { path: "home/.claude/skills/dup/SKILL.md", content: skillFile("existing", "duplicate name") },
  { path: "home/.claude/skills/no-name/SKILL.md", content: "plain body without frontmatter\n" },
  { path: "home/.mode/skills/existing/SKILL.md", content: skillFile("existing", "mode existing") },
  { path: "ws/.claude/skills/wsone/SKILL.md", content: skillFile("wsone", "workspace claude") },
  {
    path: "home/.claude/commands/deploy.md",
    content: "---\ndescription: deploy it\nargument-hint: <env>\n---\nrun deploy\n",
  },
  {
    path: "ws/.claude/commands/project.md",
    content: "---\ndescription: project command\n---\nrun project\n",
  },
  {
    path: "home/.claude/plugins/myplugin/.claude-plugin/plugin.json",
    content: JSON.stringify({ name: "myplugin", version: "1.2.0" }),
  },
  {
    path: "home/.claude/settings.json",
    content: JSON.stringify(
      { mcpServers: { srv: { command: "npx", args: ["-y", "pkg"], timeout: 5 } } },
      null,
      2,
    ),
  },
  {
    path: "home/.codex/config.toml",
    content: '[mcp_servers.tomlserver]\ncommand = "node"\nargs = ["a.js"]\n',
  },
  {
    path: "ws/.mcp.json",
    content: JSON.stringify({ mcpServers: { proj: { command: "x" } } }, null, 2),
  },
  { path: "home/.claude/CLAUDE.md", content: "claude agents body\n" },
];

async function main(): Promise<void> {
  process.env.HOME = HOME;
  process.env.USERPROFILE = HOME;
  const { createSettingsSyncService } =
    await import("../packages/services/src/settings-sync/settingsSyncService.js");

  const cases: FixtureCase[] = [
    await runCase(createSettingsSyncService, {
      name: "detect-skills",
      files: FILES,
      ops: [
        { kind: "detect", categories: ["skills"], intent: "manualImport" },
        { kind: "detect", categories: ["skills"], intent: "manualImport", useWorkspace: true },
        { kind: "detect", categories: ["skills"], intent: "firstRun" },
      ],
    }),
    await runCase(createSettingsSyncService, {
      name: "detect-commands",
      files: FILES,
      ops: [
        { kind: "detect", categories: ["commands"], intent: "manualImport" },
        { kind: "detect", categories: ["commands"], intent: "manualImport", useWorkspace: true },
      ],
    }),
    await runCase(createSettingsSyncService, {
      name: "detect-plugins-mcp",
      files: FILES,
      ops: [
        { kind: "detect", categories: ["plugins"], intent: "manualImport" },
        { kind: "detect", categories: ["mcpServers"], intent: "manualImport" },
        { kind: "detect", categories: ["mcpServers"], intent: "manualImport", useWorkspace: true },
      ],
    }),
    await runCase(createSettingsSyncService, {
      name: "import-copy",
      files: FILES,
      ops: [
        {
          kind: "importSelected",
          selections: [
            {
              agent: "claudeCode",
              category: "skills",
              importMode: "copy",
              skillPaths: ["<root>/home/.claude/skills/alpha"],
            },
            {
              agent: "claudeCode",
              category: "commands",
              importMode: "copy",
              commandPaths: ["<root>/home/.claude/commands/deploy.md"],
            },
            { agent: "claudeCode", category: "plugins", importMode: "copy" },
            { agent: "claudeCode", category: "mcpServers" },
            { agent: "codexCli", category: "mcpServers" },
            { agent: "unknownAgent", category: "providers" },
          ],
        },
        { kind: "detect", categories: ["skills"], intent: "manualImport" },
        { kind: "detect", categories: ["plugins"], intent: "manualImport" },
        { kind: "detect", categories: ["mcpServers"], intent: "manualImport" },
      ],
    }),
    await runCase(createSettingsSyncService, {
      name: "import-workspace",
      files: FILES,
      ops: [
        {
          kind: "importSelected",
          useWorkspace: true,
          selections: [
            {
              agent: "claudeCode",
              category: "skills",
              sourceScope: "project",
              targetScope: "project",
              importMode: "copy",
            },
            {
              agent: "claudeCode",
              category: "commands",
              sourceScope: "project",
              targetScope: "project",
              importMode: "copy",
            },
            {
              agent: "agents",
              category: "mcpServers",
              sourceScope: "project",
              targetScope: "project",
            },
          ],
        },
      ],
    }),
    await runCase(createSettingsSyncService, {
      name: "agents-file-and-first-run",
      files: FILES,
      ops: [
        { kind: "agentsFileStatus" },
        { kind: "copyAgentsFile" },
        { kind: "agentsFileStatus" },
        { kind: "copyAgentsFile" },
        { kind: "copyAgentsFile", overwrite: true },
        { kind: "firstRunState" },
        { kind: "markFirstRun" },
        { kind: "firstRunState" },
      ],
    }),
  ];

  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(OUTPUT, `${JSON.stringify({ cases }, null, 2)}\n`);
  process.stdout.write(`wrote ${path.relative(ROOT_DIR, OUTPUT)} (${cases.length} cases)\n`);
  rmSync(SANDBOX, { recursive: true, force: true });
}

await main();
