/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明 subagent 服务用例，拆分会降低可核对性。 */
/**
 * 生成 `subagents-service` 与 TS 真源的对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/subagents/subagentsService.ts`
 * - `packages/services/src/subagents/subagentMarkdown.ts`
 *
 * ```bash
 * pnpm subagents:fixtures
 * cargo test -p subagents-service
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
const OUTPUT = path.join(ROOT_DIR, "crates", "subagents-service", "fixtures", "subagents.json");

// 临时目录在 Windows 上是 8.3 短名；服务里 realpath 会展开成长名，两侧路径文本必须一致。
const SANDBOX = realpathSync.native(mkdtempSync(path.join(tmpdir(), "zc-subagents-fixtures-")));
const HOME = path.join(SANDBOX, "home");
const WORKSPACE = path.join(SANDBOX, "ws");

interface FileSpec {
  path: string;
  content: string;
}

interface ModelSelectionLike {
  providerId: string;
  modelId: string;
  options?: { reasoningLevel?: string };
}

interface AgentConfigLike {
  name: string;
  description: string;
  systemPrompt: string;
  color?: string;
  modelSelection?: ModelSelectionLike;
  tools?: string[];
  disallowedTools?: string[];
  injectAgentsMd?: boolean;
  skills?: string[];
  permissionMode?: string;
  maxTurns?: number;
  background?: boolean;
  mcpServers?: unknown[];
}

interface FixtureOp {
  kind:
    | "list"
    | "setEnabled"
    | "setBuiltInModelOverride"
    | "setPluginAgentModelOverride"
    | "getPrimaryUserAgentsDirectory"
    | "createAgent"
    | "updateAgent"
    | "deleteAgent"
    | "prepareRuntimeState";
  mode?: "allRuntimeScopes" | "settingsUserOnly";
  agentId?: string;
  resolveName?: string;
  enabled?: boolean;
  agentName?: "general-purpose" | "Explore";
  modelSelection?: ModelSelectionLike | null;
  config?: AgentConfigLike;
  scope?: "user" | "workspace";
  useWorkspace?: boolean;
}

interface FixtureCase {
  name: string;
  desktop: boolean;
  files: FileSpec[];
  ops: FixtureOp[];
  expected: unknown[];
  tree: FileSpec[];
}

interface SubagentsServiceLike {
  list(params: {
    workspacePath: string;
    workspaceIdentity?: string;
    mode?: "allRuntimeScopes" | "settingsUserOnly";
  }): Promise<unknown>;
  setEnabled(params: { agentId: string; enabled: boolean }): Promise<void>;
  setBuiltInModelOverride(params: {
    agentName: "general-purpose" | "Explore";
    modelSelection?: ModelSelectionLike;
  }): Promise<void>;
  setPluginAgentModelOverride(params: {
    agentId: string;
    modelSelection?: ModelSelectionLike;
  }): Promise<void>;
  getPrimaryUserAgentsDirectory(params: { provider: string }): Promise<{ path: string }>;
  createAgent(params: {
    config: AgentConfigLike;
    provider: string;
    scope?: "user" | "workspace";
    workspacePath?: string;
  }): Promise<unknown>;
  updateAgent(params: {
    agentId: string;
    config: AgentConfigLike;
    oldFilePath?: string;
    provider: string;
    scope?: "user" | "workspace";
    workspacePath?: string;
  }): Promise<unknown>;
  deleteAgent(params: { agentId: string; filePath: string }): Promise<void>;
  prepareRuntimeState(): Promise<void>;
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

/** 状态文件的键序由各自 JSON 实现决定（TS 保插入序、Rust 按字母序）；对拍前统一按键排序。 */
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
          content: entry.name.endsWith("agents-state.json")
            ? normalizeJsonFile(root, raw)
            : normalizeText(root, raw),
        });
      }
    }
  };
  walk(root, "");
  return files.sort((left, right) => left.path.localeCompare(right.path));
}

function agentFile(frontmatter: string, body = "body"): string {
  return `---\n${frontmatter}\n---\n${body}\n`;
}

async function resolveAgent(
  service: SubagentsServiceLike,
  workspacePath: string,
  name: string,
): Promise<{ id: string; path: string }> {
  const listed = (await service.list({ workspacePath })) as {
    agents: Array<{ id: string; name: string; path: string }>;
  };
  const found = listed.agents.find((agent) => agent.name === name);
  if (!found) throw new Error(`agent not found by name: ${name}`);
  return { id: found.id, path: found.path };
}

async function runOp(
  service: SubagentsServiceLike,
  workspacePath: string,
  op: FixtureOp,
): Promise<unknown> {
  try {
    switch (op.kind) {
      case "list":
        return await service.list({ workspacePath, mode: op.mode });
      case "setEnabled":
        await service.setEnabled({
          agentId:
            op.agentId ?? (await resolveAgent(service, workspacePath, op.resolveName ?? "")).id,
          enabled: op.enabled ?? false,
        });
        return { ok: true };
      case "setBuiltInModelOverride":
        await service.setBuiltInModelOverride({
          agentName: op.agentName ?? "general-purpose",
          ...(op.modelSelection ? { modelSelection: op.modelSelection } : {}),
        });
        return { ok: true };
      case "setPluginAgentModelOverride":
        await service.setPluginAgentModelOverride({
          agentId: op.agentId ?? "",
          ...(op.modelSelection ? { modelSelection: op.modelSelection } : {}),
        });
        return { ok: true };
      case "getPrimaryUserAgentsDirectory":
        return await service.getPrimaryUserAgentsDirectory({ provider: "glm" });
      case "createAgent":
        return await service.createAgent({
          config: op.config!,
          provider: "glm",
          scope: op.scope ?? "user",
          ...(op.useWorkspace ? { workspacePath } : {}),
        });
      case "updateAgent": {
        const target = op.agentId
          ? { id: op.agentId, path: "" }
          : await resolveAgent(service, workspacePath, op.resolveName ?? "");
        return await service.updateAgent({
          agentId: target.id,
          config: op.config!,
          oldFilePath: target.path || undefined,
          provider: "glm",
          scope: op.scope ?? "user",
          ...(op.useWorkspace ? { workspacePath } : {}),
        });
      }
      case "deleteAgent": {
        const target = op.agentId
          ? { id: op.agentId, path: "" }
          : await resolveAgent(service, workspacePath, op.resolveName ?? "");
        await service.deleteAgent({
          agentId: target.id,
          filePath: target.path,
        });
        return { ok: true };
      }
      case "prepareRuntimeState":
        await service.prepareRuntimeState();
        return { ok: true };
      default:
        throw new Error(`unknown op: ${String((op as { kind?: string }).kind)}`);
    }
  } catch (error) {
    return { error: error instanceof Error ? error.message : String(error) };
  }
}

async function runCase(
  createSubagentsService: (options: {
    homeDir: string;
    isDesktopRuntime: boolean;
  }) => SubagentsServiceLike,
  testCase: Omit<FixtureCase, "expected" | "tree">,
): Promise<FixtureCase> {
  rmSync(SANDBOX, { recursive: true, force: true });
  mkdirSync(SANDBOX, { recursive: true });
  materialize(SANDBOX, testCase.files);

  const service = createSubagentsService({
    homeDir: HOME,
    isDesktopRuntime: testCase.desktop,
  });
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

const BASIC_FILES: FileSpec[] = [
  { path: "ws/.git/HEAD", content: "ref: refs/heads/main\n" },
  {
    path: "ws/.mode/agents/alpha.md",
    content: agentFile(
      'name: "alpha"\ndescription: "workspace alpha"\nmodel: glm/GLM-4.7\nthoughtLevel: high\ntools: Read, Grep\nmaxTurns: 3\nbackground: true',
      "alpha body",
    ),
  },
  {
    path: "ws/.mode/agents/group/nested.md",
    content: agentFile(
      'name: "nested"\ndescription: "nested agent"\nmcpServers:\n  - name: fs\n    command: npx\n    args:\n      - -y\n      - pkg',
      "nested body",
    ),
  },
  { path: "ws/.mode/agents/notes.txt", content: "not an agent\n" },
  {
    path: "home/.mode/agents/userone.md",
    content: agentFile('name: "userone"\ndescription: "user agent"', "user body"),
  },
  {
    path: "home/.mode/agents/broken.md",
    content: "---\nname: broken\n---\n",
  },
];

const PLUGIN_FILES: FileSpec[] = [
  { path: "ws/.git/HEAD", content: "ref: refs/heads/main\n" },
  {
    path: "home/.mode/cli/config.json",
    content: JSON.stringify(
      {
        plugins: {
          dirs: ["<root>/home/.mode/cli/plugins-inline"],
          enabledPlugins: {
            "plug-a@inline": true,
            "plug-b@inline": false,
            "plug-c@inline": true,
            "claude-z@mode-plugins-official": true,
          },
          suppressedBuiltins: ["builtin-x@zcode-plugins-official"],
        },
      },
      null,
      2,
    ),
  },
  {
    path: "home/.mode/cli/plugins-inline/plug-a/.mode-plugin/plugin.json",
    content: JSON.stringify({ name: "plug-a", agents: "agents" }),
  },
  {
    path: "home/.mode/cli/plugins-inline/plug-a/agents/helper.md",
    content: agentFile('name: "helper"\ndescription: "plugin helper"', "helper body"),
  },
  {
    path: "home/.mode/cli/plugins-inline/plug-b/.mode-plugin/plugin.json",
    content: JSON.stringify({ name: "plug-b" }),
  },
  {
    path: "home/.mode/cli/plugins-inline/plug-b/agents/hidden.md",
    content: agentFile('name: "hidden"\ndescription: "disabled plugin"'),
  },
  {
    path: "home/.mode/cli/plugins-inline/plug-c/.mode-plugin/plugin.json",
    content: JSON.stringify({ name: "plug-c" }),
  },
  {
    path: "home/.mode/cli/plugins-inline/plug-c/agents/helper.md",
    content: agentFile('name: "helper"\ndescription: "second helper"'),
  },
  {
    path: "home/.mode/cli/plugins/installed_plugins.json",
    content: JSON.stringify({
      plugins: [
        {
          id: "claude-z@zcode-plugins-official",
          marketplace: "zcode-plugins-official",
          installPath: "<root>/home/installed/claude-z",
          name: "claude-z",
        },
      ],
    }),
  },
  {
    path: "home/installed/claude-z/.claude-plugin/plugin.json",
    content: JSON.stringify({ name: "claude-z", agents: ["agents", "./extra"] }),
  },
  {
    path: "home/installed/claude-z/agents/seven.md",
    content: agentFile('name: "seven"\ndescription: "installed plugin agent"'),
  },
  {
    path: "home/installed/claude-z/extra/eight.md",
    content: agentFile('name: "eight"\ndescription: "installed extra agent"'),
  },
];

const MIGRATION_FILES: FileSpec[] = [
  { path: "ws/.git/HEAD", content: "ref: refs/heads/main\n" },
  {
    path: "home/.mode/v2/agents-state.json",
    content: JSON.stringify(
      {
        builtInModelOverrides: {
          Explore: "builtin:zai/glm-4.7",
          "general-purpose": "builtin:bigmodel/GLM-4.6",
        },
        builtInThoughtLevelOverrides: { Explore: "high" },
        pluginAgentModelOverrides: { "plugin:p@inline:x": "builtin:zai/glm-4.7" },
        pluginAgentThoughtLevelOverrides: { "plugin:p@inline:x": "low" },
        disabledAgentIds: ["user:user:legacy"],
      },
      null,
      2,
    ),
  },
  {
    path: "home/.mode/agents/legacy.md",
    content: agentFile(
      'name: "legacy"\ndescription: "legacy model"\nmodel: "builtin:zai/glm-4.7$high"',
      "legacy body",
    ),
  },
  {
    path: "home/.mode/agents/plain.md",
    content: agentFile('name: "plain"\ndescription: "no model"'),
  },
];

async function main(): Promise<void> {
  const { createSubagentsService } =
    await import("../packages/services/src/subagents/subagentsService.js");

  const cases: FixtureCase[] = [
    await runCase(createSubagentsService, {
      name: "list-basic",
      desktop: true,
      files: BASIC_FILES,
      ops: [{ kind: "list" }],
    }),
    await runCase(createSubagentsService, {
      name: "list-cli",
      desktop: false,
      files: BASIC_FILES,
      ops: [{ kind: "list" }],
    }),
    await runCase(createSubagentsService, {
      name: "settings-user-only",
      desktop: true,
      files: BASIC_FILES,
      ops: [{ kind: "list", mode: "settingsUserOnly" }, { kind: "list" }],
    }),
    await runCase(createSubagentsService, {
      name: "state-mutations",
      desktop: true,
      files: BASIC_FILES,
      ops: [
        { kind: "list" },
        { kind: "setEnabled", resolveName: "userone", enabled: false },
        { kind: "list" },
        { kind: "setEnabled", resolveName: "userone", enabled: true },
        {
          kind: "setBuiltInModelOverride",
          agentName: "general-purpose",
          modelSelection: {
            providerId: "glm",
            modelId: "GLM-5",
            options: { reasoningLevel: "high" },
          },
        },
        {
          kind: "setBuiltInModelOverride",
          agentName: "Explore",
          modelSelection: { providerId: "zai-api", modelId: "GLM-4.7" },
        },
        {
          kind: "setPluginAgentModelOverride",
          agentId: "plugin:plug@inline:helper",
          modelSelection: { providerId: "glm", modelId: "GLM-5" },
        },
        { kind: "setPluginAgentModelOverride", agentId: "not-a-plugin-id" },
        { kind: "list" },
        { kind: "setBuiltInModelOverride", agentName: "general-purpose" },
        { kind: "setPluginAgentModelOverride", agentId: "plugin:plug@inline:helper" },
        { kind: "getPrimaryUserAgentsDirectory" },
      ],
    }),
    await runCase(createSubagentsService, {
      name: "create-update-delete",
      desktop: true,
      files: BASIC_FILES,
      ops: [
        {
          kind: "createAgent",
          scope: "workspace",
          useWorkspace: true,
          config: {
            name: "new-one",
            description: "created",
            systemPrompt: "do the thing",
            color: "blue",
            modelSelection: { providerId: "glm", modelId: "GLM-5" },
            tools: ["Read", "Grep"],
            skills: ["alpha"],
            permissionMode: "auto",
            maxTurns: 2,
            background: false,
            injectAgentsMd: true,
          },
        },
        { kind: "list" },
        {
          kind: "createAgent",
          scope: "workspace",
          useWorkspace: true,
          config: { name: "new-one", description: "dup", systemPrompt: "dup" },
        },
        {
          kind: "createAgent",
          config: { name: "ab", description: "short", systemPrompt: "short" },
        },
        {
          kind: "createAgent",
          config: { name: "Explore", description: "reserved", systemPrompt: "reserved" },
        },
        {
          kind: "updateAgent",
          resolveName: "new-one",
          scope: "workspace",
          useWorkspace: true,
          config: {
            name: "renamed",
            description: "renamed desc",
            systemPrompt: "renamed prompt",
            tools: ["Read"],
          },
        },
        { kind: "list" },
        { kind: "deleteAgent", resolveName: "renamed" },
        { kind: "list" },
      ],
    }),
    await runCase(createSubagentsService, {
      name: "plugin-agents",
      desktop: true,
      files: PLUGIN_FILES,
      ops: [{ kind: "list" }, { kind: "list", mode: "settingsUserOnly" }],
    }),
    await runCase(createSubagentsService, {
      name: "migrations",
      desktop: true,
      files: MIGRATION_FILES,
      ops: [{ kind: "prepareRuntimeState" }, { kind: "list" }],
    }),
  ];

  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(OUTPUT, `${JSON.stringify({ cases }, null, 2)}\n`);
  process.stdout.write(`wrote ${path.relative(ROOT_DIR, OUTPUT)} (${cases.length} cases)\n`);
  rmSync(SANDBOX, { recursive: true, force: true });
}

await main();
