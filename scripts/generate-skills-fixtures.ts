/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明技能服务用例，拆分会降低可核对性。 */
/**
 * 生成 `skills-service` 与 TS 真源的对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/skills/skillsService.ts`
 * - `packages/services/src/skills/skillDiscoveryWalk.ts`
 *
 * ```bash
 * pnpm skills:fixtures
 * cmd //c "D:\\tools\\cargo-msvc.cmd test -p skills-service"
 * ```
 *
 * 注意：`skillsService.ts` 在模块加载时用 `process.env.HOME` 固定 CLI config 路径，
 * 所以必须先设置环境变量再动态 import，且所有用例共用同一个临时根（每例清空重建）。
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
const OUTPUT = path.join(ROOT_DIR, "crates", "skills-service", "fixtures", "skills.json");

// `os.tmpdir()` 在 Windows 上返回 8.3 短路径；`realpathSync` 不会展开短名，
// 服务里异步的 `realpath()` 却会展开成 `C:\Users\Administrator\...`。
// 两侧形式不同会让路径替换与包含判断全部失效，因此这里用 native 版拿长路径。
const SANDBOX = realpathSync.native(mkdtempSync(path.join(tmpdir(), "zc-skills-fixtures-")));
const HOME = path.join(SANDBOX, "home");
const WORKSPACE = path.join(SANDBOX, "ws");

interface FileSpec {
  path: string;
  content: string;
}

interface FixtureOp {
  kind:
    | "list"
    | "setEnabled"
    | "buildPromptContext"
    | "copyToCommon"
    | "removeFromCommon"
    | "deleteSkill";
  name?: string;
  skillId?: string;
  enabled?: boolean;
  prompt?: string;
}

interface FixtureCase {
  name: string;
  desktop: boolean;
  files: FileSpec[];
  ops: FixtureOp[];
  expected: unknown[];
  tree: FileSpec[];
}

interface SkillsServiceLike {
  list(params: { workspacePath: string }): Promise<unknown>;
  setEnabled(params: { workspacePath: string; skillId: string; enabled: boolean }): Promise<void>;
  buildPromptContext(params: { workspacePath: string; prompt: string }): Promise<unknown>;
  copyToCommon(params: { workspacePath: string; skillId: string }): Promise<unknown>;
  removeFromCommon(params: { workspacePath: string; skillId: string }): Promise<void>;
  deleteSkill(params: { workspacePath: string; skillId: string }): Promise<void>;
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

function normalizeSkillId(id: string): string {
  const match = /^(glm:[a-z]+:.+):[0-9a-f]{12}$/u.exec(id);
  return match ? `${match[1]}:<id-hash>` : id;
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
  if (typeof value === "string") {
    return normalizeText(root, value);
  }
  if (Array.isArray(value)) {
    return value.map((item) => normalizeValue(root, item));
  }
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [key, child] of Object.entries(value as Record<string, unknown>)) {
      out[key] =
        key === "id" && typeof child === "string"
          ? normalizeSkillId(child)
          : normalizeValue(root, child);
    }
    return out;
  }
  return value;
}

function normalizeAuditLog(root: string, content: string): string {
  return content
    .split("\n")
    .filter((line) => line.length > 0)
    .map((line) => {
      const record = JSON.parse(line) as Record<string, unknown>;
      record.createdAt = "<ts>";
      const ordered: Record<string, unknown> = {};
      for (const key of Object.keys(record).sort()) {
        ordered[key] = record[key];
      }
      return normalizeText(root, JSON.stringify(ordered));
    })
    .join("\n");
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
          content: entry.name.endsWith("skills-audit.log")
            ? normalizeAuditLog(root, raw)
            : normalizeText(root, raw),
        });
      }
    }
  };
  walk(root, "");
  return files.sort((left, right) => left.path.localeCompare(right.path));
}

function skillFile(
  name: string,
  description: string,
  body = `${name} body`,
  extra = "",
): FileSpec["content"] {
  return `---\nname: ${name}\ndescription: ${description}\n${extra}---\n${body}\n`;
}

const LONG_DESCRIPTION = "x".repeat(1100);

const WORKSPACE_FILES: FileSpec[] = [
  { path: "ws/.git/HEAD", content: "ref: refs/heads/main\n" },
  { path: "ws/.mode/skills/alpha/SKILL.md", content: skillFile("alpha", "plain description") },
  {
    path: "ws/.mode/skills/group/nested/SKILL.md",
    content: skillFile("nested", "|\n  line one\n  line two"),
  },
  {
    path: "ws/.mode/skills/group/node_modules/pkg/SKILL.md",
    content: skillFile("excluded", "should not be scanned"),
  },
  {
    path: "ws/.mode/skills/.hidden/SKILL.md",
    content: skillFile("hidden", "dot directories are skipped"),
  },
  { path: "ws/.mode/skills/.system/sys/SKILL.md", content: skillFile("sys", "system skill") },
  {
    path: "ws/.mode/skills/crlf/SKILL.md",
    content: "---\r\nname: \"crlf\"\r\ndescription: 'quoted value'\r\n---\r\n\r\ncrlf body\r\n",
  },
  {
    path: "ws/.mode/skills/crlf/_meta.json",
    content: JSON.stringify({ slug: " crlf-slug ", version: "1.2.3", publishedAt: 1700000000 }),
  },
  { path: "ws/.mode/skills/nofm/SKILL.md", content: "no frontmatter here\n" },
  {
    path: "ws/.mode/skills/leak/SKILL.md",
    content: "---\nname: leak\n# Usage\n\nleaked body line\n---\n",
  },
  {
    path: "ws/.mode/skills/chinese/SKILL.md",
    content: skillFile("chinese", "触发场景: 中文说明"),
  },
  {
    path: "ws/.mode/skills/toobig/SKILL.md",
    content: skillFile("toobig", LONG_DESCRIPTION),
  },
  {
    path: "ws/.mode/skills/badmeta/SKILL.md",
    content: skillFile("badmeta", "broken metadata"),
  },
  { path: "ws/.mode/skills/badmeta/_meta.json", content: "{ not json" },
  { path: "ws/.agents/skills/beta/SKILL.md", content: skillFile("beta", "agents workspace skill") },
];

const USER_FILES: FileSpec[] = [
  { path: "home/.mode/skills/dup/SKILL.md", content: skillFile("shared", "user mode skill") },
  {
    path: "home/.agents/skills/dup2/SKILL.md",
    content: skillFile("shared", "user agents duplicate"),
  },
  {
    path: "home/.agents/skills/only/SKILL.md",
    content: skillFile("agentsonly", "user agents only"),
  },
  { path: "home/.agents/skills/shared/SKILL.md", content: skillFile("shared", "same folder name") },
  {
    path: "home/.mode/cli/config.json",
    content: JSON.stringify(
      {
        theme: "dark",
        skills: { "<root>/home/.mode/skills/dup/SKILL.md": { enable: false } },
      },
      null,
      2,
    ),
  },
];

const PLUGIN_FILES: FileSpec[] = [
  {
    path: "home/.mode/cli/config.json",
    content: JSON.stringify(
      {
        plugins: {
          dirs: ["<root>/home/.mode/cli/plugins-inline"],
          enabled: true,
          enabledPlugins: { "plug-b@inline": false, "plug-c@zcode-plugins-official": true },
          suppressedBuiltins: ["builtin-x@zcode-plugins-official"],
        },
      },
      null,
      2,
    ),
  },
  {
    path: "home/.mode/cli/plugins-inline/plug-a/.mode-plugin/plugin.json",
    content: JSON.stringify({ name: "plug-a", skills: "skills" }),
  },
  {
    path: "home/.mode/cli/plugins-inline/plug-a/skills/one/SKILL.md",
    content: skillFile("one", "inline plugin skill"),
  },
  {
    path: "home/.mode/cli/plugins-inline/plug-a/skills/escape/SKILL.md",
    content: skillFile("escape", "not part of the configured root"),
  },
  {
    path: "home/.mode/cli/plugins-inline/plug-b/.mode-plugin/plugin.json",
    content: JSON.stringify({ name: "plug-b" }),
  },
  {
    path: "home/.mode/cli/plugins-inline/plug-b/skills/two/SKILL.md",
    content: skillFile("two", "disabled plugin skill"),
  },
  {
    path: "home/.mode/cli/plugins-inline/plug-bad/.mode-plugin/plugin.json",
    content: JSON.stringify({ name: "Plug-Bad" }),
  },
  {
    path: "home/.mode/cli/plugins-inline/plug-bad/skills/three/SKILL.md",
    content: skillFile("three", "invalid plugin name"),
  },
  {
    path: "home/.mode/cli/plugins/cache/mode-plugins-official/builtin-y/1.0.0/.mode-plugin/plugin.json",
    content: JSON.stringify({ name: "builtin-y", skills: ["skills"] }),
  },
  {
    path: "home/.mode/cli/plugins/cache/mode-plugins-official/builtin-y/1.0.0/skills/four/SKILL.md",
    content: skillFile("four", "official cache skill"),
  },
  {
    path: "home/.mode/cli/plugins/cache/mode-plugins-official/builtin-y/2.0.0/.mode-plugin/plugin.json",
    content: JSON.stringify({ name: "builtin-y", skills: ["skills"] }),
  },
  {
    path: "home/.mode/cli/plugins/cache/mode-plugins-official/builtin-y/2.0.0/skills/five/SKILL.md",
    content: skillFile("five", "newer version skill"),
  },
  {
    path: "home/.mode/cli/plugins/cache/mode-plugins-official/builtin-x/1.0.0/.mode-plugin/plugin.json",
    content: JSON.stringify({ name: "builtin-x", skills: ["skills"] }),
  },
  {
    path: "home/.mode/cli/plugins/cache/mode-plugins-official/builtin-x/1.0.0/skills/six/SKILL.md",
    content: skillFile("six", "suppressed builtin"),
  },
  {
    path: "home/.mode/cli/plugins/installed_plugins.json",
    content: JSON.stringify({
      plugins: [
        {
          id: "claude-z@zcode-plugins-official",
          marketplace: "zcode-plugins-official",
          installPath: "<root>/home/installed/claude-z",
        },
        { id: "relative", marketplace: "inline", installPath: "relative/path" },
      ],
    }),
  },
  {
    path: "home/installed/claude-z/.claude-plugin/plugin.json",
    content: JSON.stringify({ name: "claude-z", skills: "skills" }),
  },
  {
    path: "home/installed/claude-z/skills/seven/SKILL.md",
    content: skillFile("seven", "installed marketplace skill"),
  },
];

const MUTATION_FILES: FileSpec[] = [
  { path: "ws/.git/HEAD", content: "ref: refs/heads/main\n" },
  { path: "ws/.mode/skills/alpha/SKILL.md", content: skillFile("alpha", "workspace alpha") },
  { path: "ws/.mode/skills/hard/SKILL.md", content: skillFile("hard", "workspace deletable") },
  { path: "ws/.agents/skills/beta/SKILL.md", content: skillFile("beta", "workspace agents beta") },
  {
    path: "home/.mode/skills/userone/SKILL.md",
    content: skillFile("userone", "user common skill"),
  },
  { path: "home/.agents/skills/agentone/SKILL.md", content: skillFile("agentone", "agents skill") },
];

async function resolveSkillId(
  service: SkillsServiceLike,
  workspacePath: string,
  name: string,
): Promise<string> {
  const listed = (await service.list({ workspacePath })) as {
    skills: Array<{ id: string; name: string }>;
  };
  const found = listed.skills.find((skill) => skill.name === name);
  if (!found) {
    throw new Error(`skill not found by name: ${name}`);
  }
  return found.id;
}

async function runOp(
  service: SkillsServiceLike,
  workspacePath: string,
  op: FixtureOp,
): Promise<unknown> {
  try {
    switch (op.kind) {
      case "list":
        return await service.list({ workspacePath });
      case "buildPromptContext":
        return await service.buildPromptContext({ workspacePath, prompt: op.prompt ?? "" });
      case "setEnabled": {
        const skillId = op.skillId ?? (await resolveSkillId(service, workspacePath, op.name ?? ""));
        await service.setEnabled({ workspacePath, skillId, enabled: op.enabled ?? false });
        return { ok: true };
      }
      case "copyToCommon": {
        const skillId = op.skillId ?? (await resolveSkillId(service, workspacePath, op.name ?? ""));
        return await service.copyToCommon({ workspacePath, skillId });
      }
      case "removeFromCommon": {
        const skillId = op.skillId ?? (await resolveSkillId(service, workspacePath, op.name ?? ""));
        await service.removeFromCommon({ workspacePath, skillId });
        return { ok: true };
      }
      case "deleteSkill": {
        const skillId = op.skillId ?? (await resolveSkillId(service, workspacePath, op.name ?? ""));
        await service.deleteSkill({ workspacePath, skillId });
        return { ok: true };
      }
      default:
        throw new Error(`unknown op: ${String(op.kind)}`);
    }
  } catch (error) {
    return { error: error instanceof Error ? error.message : String(error) };
  }
}

async function runCase(
  createSkillsService: (options: { isDesktopRuntime: boolean }) => SkillsServiceLike,
  testCase: Omit<FixtureCase, "expected" | "tree">,
): Promise<FixtureCase> {
  rmSync(SANDBOX, { recursive: true, force: true });
  mkdirSync(SANDBOX, { recursive: true });
  materialize(SANDBOX, testCase.files);

  const service = createSkillsService({ isDesktopRuntime: testCase.desktop });
  const expected: unknown[] = [];
  for (const op of testCase.ops) {
    expected.push(normalizeValue(SANDBOX, await runOp(service, WORKSPACE, op)));
  }
  return {
    name: testCase.name,
    desktop: testCase.desktop,
    files: testCase.files,
    ops: testCase.ops,
    expected,
    tree: captureTree(SANDBOX),
  };
}

async function main(): Promise<void> {
  process.env.HOME = HOME;
  process.env.USERPROFILE = HOME;
  delete process.env.MODE_PROCESS_LABEL;

  const { createSkillsService } = await import("../packages/services/src/skills/skillsService.js");

  const cases: FixtureCase[] = [
    await runCase(createSkillsService, {
      name: "workspace-discovery",
      desktop: true,
      files: WORKSPACE_FILES,
      ops: [{ kind: "list" }],
    }),
    await runCase(createSkillsService, {
      name: "user-scope",
      desktop: true,
      files: [...WORKSPACE_FILES, ...USER_FILES],
      ops: [{ kind: "list" }],
    }),
    await runCase(createSkillsService, {
      name: "user-scope-disabled-runtime",
      desktop: false,
      files: [...WORKSPACE_FILES, ...USER_FILES],
      ops: [{ kind: "list" }],
    }),
    await runCase(createSkillsService, {
      name: "plugin-roots",
      desktop: true,
      files: PLUGIN_FILES,
      ops: [{ kind: "list" }, { kind: "deleteSkill", name: "one" }],
    }),
    await runCase(createSkillsService, {
      name: "mutations",
      desktop: true,
      files: MUTATION_FILES,
      ops: [
        { kind: "list" },
        { kind: "setEnabled", name: "alpha", enabled: false },
        { kind: "list" },
        { kind: "buildPromptContext", prompt: "please use $alpha and $userone" },
        { kind: "copyToCommon", name: "agentone" },
        { kind: "removeFromCommon", name: "userone" },
        { kind: "deleteSkill", name: "hard" },
        { kind: "removeFromCommon", name: "beta" },
        { kind: "setEnabled", skillId: "glm:user:ghost:000000000000", enabled: true },
        { kind: "list" },
      ],
    }),
  ];

  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  const payload = { cases };
  writeFileSync(OUTPUT, `${JSON.stringify(payload, null, 2)}\n`);
  process.stdout.write(`wrote ${path.relative(ROOT_DIR, OUTPUT)} (${cases.length} cases)\n`);
  rmSync(SANDBOX, { recursive: true, force: true });
}

await main();
