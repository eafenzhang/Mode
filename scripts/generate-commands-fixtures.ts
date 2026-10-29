/* eslint-disable max-lines -- 场景数据（parser/generator/list/override）集中在一处才能看清 TS 与 Rust 的对拍口径，拆分会打散同一份契约。 */
/**
 * 生成 `commands` 域的 Rust 对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/commands/commandFileParser.ts`
 * - `packages/services/src/commands/commandsService.ts`
 * - `packages/services/src/plugins/installedPluginRoots.ts`
 *
 * ```bash
 * pnpm commands:fixtures
 * cargo test -p commands-service
 * ```
 *
 * 绝对路径统一用 `<ROOT>` 占位并归一为 `/` 分隔，Rust 侧在临时目录重建同一棵
 * 文件树后做同样替换再比较（fixture 因此可跨平台复用）。
 */
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { CommandFileParser } from "../packages/services/src/commands/commandFileParser.js";
import { createCommandsService } from "../packages/services/src/commands/commandsService.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT_DIR, "crates", "commands-service", "fixtures", "commands.json");
const ROOT_TOKEN = "<ROOT>";

type Json = unknown;

interface ListScenario {
  id: string;
  dirs?: string[];
  files?: Array<[string, string]>;
  /**
   * JSON 文件按结构保存：写盘前先替换路径模板再 `JSON.stringify`，避免把
   * 反斜杠塞进已经序列化好的字符串里破坏 JSON 转义。
   */
  jsonFiles?: Array<[string, Json]>;
  /** 配置模板：字符串里的 `<ROOT>` 会在写盘前替换成实际临时根。 */
  config?: Record<string, Json>;
  listParams?: { workspacePath?: string; agentSource?: "modeAgent" };
}

const parserCases: Array<{ content: string; filePath: string; expected: Json }> = [
  {
    content: "just a body\n",
    filePath: "/ws/.mode/commands/simple.md",
    expected: CommandFileParser.parseCommandFile("just a body\n", "/ws/.mode/commands/simple.md"),
  },
  {
    content: "---\ndescription: hello\nargument-hint: [name]\n---\n\nbody line\nsecond\n",
    filePath: "/ws/.mode/commands/front.md",
    expected: CommandFileParser.parseCommandFile(
      "---\ndescription: hello\nargument-hint: [name]\n---\n\nbody line\nsecond\n",
      "/ws/.mode/commands/front.md",
    ),
  },
  {
    content: "---\ndescription: hello: world\n---\nbody",
    filePath: "/ws/.mode/commands/colon.md",
    expected: CommandFileParser.parseCommandFile(
      "---\ndescription: hello: world\n---\nbody",
      "/ws/.mode/commands/colon.md",
    ),
  },
  {
    content: "---\nargument-hint: |\n  first\n  second\nother: x\n---\nbody",
    filePath: "/ws/.mode/commands/multiline.md",
    expected: CommandFileParser.parseCommandFile(
      "---\nargument-hint: |\n  first\n  second\nother: x\n---\nbody",
      "/ws/.mode/commands/multiline.md",
    ),
  },
  {
    content: "intro line\n---\ndescription: d\n---\nbody",
    filePath: "/ws/.mode/commands/intro.md",
    expected: CommandFileParser.parseCommandFile(
      "intro line\n---\ndescription: d\n---\nbody",
      "/ws/.mode/commands/intro.md",
    ),
  },
  {
    content: "  ---  \r\ndescription: d\r\n  ---\r\nbody\r\n",
    filePath: "/ws/.mode/commands/indented.md",
    expected: CommandFileParser.parseCommandFile(
      "  ---  \r\ndescription: d\r\n  ---\r\nbody\r\n",
      "/ws/.mode/commands/indented.md",
    ),
  },
  {
    content: "---\nDescription: upper\nArgument-Hint: x\n---\nbody",
    filePath: "/ws/.mode/commands/upper.md",
    expected: CommandFileParser.parseCommandFile(
      "---\nDescription: upper\nArgument-Hint: x\n---\nbody",
      "/ws/.mode/commands/upper.md",
    ),
  },
  {
    content: "---\ndescription: unclosed\nbody",
    filePath: "/ws/.mode/commands/unclosed.md",
    expected: CommandFileParser.parseCommandFile(
      "---\ndescription: unclosed\nbody",
      "/ws/.mode/commands/unclosed.md",
    ),
  },
  {
    content: "---\ndescription:\n---\nbody",
    filePath: "/ws/.mode/commands/empty-desc.md",
    expected: CommandFileParser.parseCommandFile(
      "---\ndescription:\n---\nbody",
      "/ws/.mode/commands/empty-desc.md",
    ),
  },
  {
    content: "body",
    filePath: "C:\\ws\\.mode\\commands\\WIN.MD",
    expected: CommandFileParser.parseCommandFile("body", "C:\\ws\\.mode\\commands\\WIN.MD"),
  },
];

const generatorCases: Array<{ config: Json; existing?: string; expected: string }> = [
  {
    config: { name: "/a", prompt: "  raw prompt\n" },
    expected: CommandFileParser.generateCommandFileContent({ name: "/a", prompt: "  raw prompt\n" }),
  },
  {
    config: { name: "/a", prompt: "body", description: "  desc  ", argumentHint: " [x] " },
    expected: CommandFileParser.generateCommandFileContent({
      name: "/a",
      prompt: "body",
      description: "  desc  ",
      argumentHint: " [x] ",
    }),
  },
  {
    config: { name: "/a", prompt: "new body", description: "new" },
    existing:
      "---\nallowed-tools: Bash\n  continuation\ndescription: old\nargument-hint: old\n---\nold body",
    expected: CommandFileParser.generateCommandFileContent(
      { name: "/a", prompt: "new body", description: "new" },
      "markdown",
      "---\nallowed-tools: Bash\n  continuation\ndescription: old\nargument-hint: old\n---\nold body",
    ),
  },
  {
    config: { name: "/a", prompt: "body", description: "   ", argumentHint: "" },
    expected: CommandFileParser.generateCommandFileContent({
      name: "/a",
      prompt: "body",
      description: "   ",
      argumentHint: "",
    }),
  },
  {
    config: { name: "/a", prompt: "body" },
    existing: "intro\n---\nallowed-tools: Bash\n  keep line\n---\nold",
    expected: CommandFileParser.generateCommandFileContent(
      { name: "/a", prompt: "body" },
      "markdown",
      "intro\n---\nallowed-tools: Bash\n  keep line\n---\nold",
    ),
  },
  {
    config: { name: "/a", prompt: "body", description: "d" },
    existing: "---\npre-key: value\n  stray\nDescription: OLD\n---\nold",
    expected: CommandFileParser.generateCommandFileContent(
      { name: "/a", prompt: "body", description: "d" },
      "markdown",
      "---\npre-key: value\n  stray\nDescription: OLD\n---\nold",
    ),
  },
];

const fileNameCases: Array<{ name: string; expected: string }> = [
  { name: "/a", expected: "a.md" },
  { name: "a/b", expected: "a/b.md" },
  { name: "//a", expected: "/a.md" },
  { name: "/nested/deep", expected: "nested/deep.md" },
  { name: "/A.MD", expected: "A.MD.md" },
];

const listScenarios: ListScenario[] = [
  {
    id: "priority-and-scope-order",
    files: [
      ["home/.mode/commands/c.md", "user mode"],
      ["home/.agents/commands/d.md", "user agents"],
      ["ws/.mode/commands/a.md", "---\ndescription: project a\n---\n\nmode body"],
      ["ws/.agents/commands/b.md", "agents body"],
    ],
    listParams: { workspacePath: "ws" },
  },
  {
    id: "agents-fallback",
    dirs: ["ws/.mode/commands"],
    files: [
      ["ws/.agents/commands/nested/b.md", "project agents nested"],
      ["home/.mode/commands/g.md", "global mode"],
    ],
    listParams: { workspacePath: "ws" },
  },
  {
    id: "dedupe-first-wins",
    files: [
      ["ws/.mode/commands/same.md", "project same"],
      ["home/.mode/commands/same.md", "global same"],
    ],
    listParams: { workspacePath: "ws" },
  },
  {
    id: "enable-overrides",
    files: [
      ["home/.mode/commands/off.md", "off body"],
      ["home/.mode/commands/on.md", "on body"],
    ],
    config: {
      command: {
        "<ROOT>/home/.mode/commands/off.md": { enable: false },
        "<ROOT>/home/.mode/commands/not-a-bool.md": { enable: "no" },
      },
      theme: "dark",
    },
  },
  {
    id: "plugins-inline-with-override",
    files: [
      ["home/.mode/commands/u.md", "user body"],
      ["plugin-a/.mode-plugin/plugin.json", JSON.stringify({ name: "plugin-a", commands: "commands" })],
      ["plugin-a/commands/alpha.md", "---\ndescription: alpha\n---\n\nalpha body"],
      ["plugin-a/commands/nested/beta.md", "beta body"],
      ["plugin-a/commands/notes.txt", "ignored"],
    ],
    config: {
      plugins: { dirs: ["<ROOT>/plugin-a"] },
      command: { "<ROOT>/plugin-a/commands/alpha.md": { enable: false } },
    },
  },
  {
    id: "plugins-disabled",
    files: [
      ["plugin-b/.mode-plugin/plugin.json", JSON.stringify({ name: "plugin-b", commands: "commands" })],
      ["plugin-b/commands/x.md", "x body"],
    ],
    config: { plugins: { dirs: ["<ROOT>/plugin-b"], enabled: false } },
  },
  {
    id: "plugin-default-commands-dir",
    files: [
      ["plugin-c/.mode-plugin/plugin.json", JSON.stringify({ name: "plugin-c" })],
      ["plugin-c/commands/x.md", "x body"],
    ],
    config: { plugins: { dirs: ["<ROOT>/plugin-c"] } },
  },
  {
    id: "official-cache-default-and-enabled",
    files: [
      [
        "home/.mode/cli/plugins/cache/mode-plugins-official/demo/1.0.0/.mode-plugin/plugin.json",
        JSON.stringify({ name: "demo", commands: "commands" }),
      ],
      [
        "home/.mode/cli/plugins/cache/mode-plugins-official/demo/1.0.0/commands/export.md",
        "demo body",
      ],
      [
        "home/.mode/cli/plugins/cache/mode-plugins-official/pdf/2.0.0/.mode-plugin/plugin.json",
        JSON.stringify({ name: "pdf", commands: "commands" }),
      ],
      [
        "home/.mode/cli/plugins/cache/mode-plugins-official/pdf/2.0.0/commands/render.md",
        "pdf body",
      ],
    ],
    config: { plugins: { enabledPlugins: { "demo@mode-plugins-official": true } } },
  },
  {
    id: "suppressed-builtins-canonicalized",
    files: [
      [
        "home/.mode/cli/plugins/cache/mode-plugins-official/demo/1.0.0/.mode-plugin/plugin.json",
        JSON.stringify({ name: "demo", commands: "commands" }),
      ],
      [
        "home/.mode/cli/plugins/cache/mode-plugins-official/demo/1.0.0/commands/export.md",
        "demo body",
      ],
    ],
    config: {
      plugins: {
        enabledPlugins: { "demo@mode-plugins-official": true },
        suppressedBuiltins: ["demo@zcode-plugins-official"],
      },
    },
  },
  {
    id: "installed-plugins-json",
    files: [
      ["installed/.mode-plugin/plugin.json", JSON.stringify({ name: "installed", commands: "commands" })],
      ["installed/commands/run.md", "run body"],
    ],
    jsonFiles: [
      [
        "home/.mode/cli/plugins/installed_plugins.json",
        {
          plugins: [
            {
              id: "installed@zcode-plugins-official",
              marketplace: "zcode-plugins-official",
              installPath: "<ROOT>/installed",
            },
            { id: "relative", marketplace: "m", installPath: "relative/path" },
          ],
        },
      ],
    ],
    config: {
      plugins: { enabledPlugins: { "installed@mode-plugins-official": true } },
    },
  },
  {
    id: "custom-storage-dir",
    files: [
      [
        "store/cli/plugins/cache/mode-plugins-official/demo/1.0.0/.mode-plugin/plugin.json",
        JSON.stringify({ name: "demo", commands: "commands" }),
      ],
      [
        "store/cli/plugins/cache/mode-plugins-official/demo/1.0.0/commands/export.md",
        "demo body",
      ],
    ],
    config: {
      storage: { dir: "<ROOT>/store" },
      plugins: { enabledPlugins: { "demo@mode-plugins-official": true } },
    },
  },
  {
    id: "agent-source-filter-keeps-plugins",
    files: [
      ["home/.mode/commands/u.md", "u body"],
      ["plugin-d/.mode-plugin/plugin.json", JSON.stringify({ name: "plugin-d", commands: "commands" })],
      ["plugin-d/commands/z.md", "z body"],
    ],
    config: { plugins: { dirs: ["<ROOT>/plugin-d"] } },
    listParams: { agentSource: "modeAgent" },
  },
];

const overrideCases: Array<{
  id: string;
  initial?: Record<string, Json>;
  calls: Array<{ filePath: string; enabled: boolean }>;
}> = [
  {
    id: "disable-then-enable-drops-entry",
    initial: { theme: "dark" },
    calls: [
      { filePath: "<ROOT>/home/.mode/commands/a.md", enabled: false },
      { filePath: "<ROOT>/home/.mode/commands/a.md", enabled: true },
    ],
  },
  {
    id: "preserves-unrelated-entries",
    initial: { command: { "<ROOT>/home/.mode/commands/other.md": { enable: false } }, theme: "light" },
    calls: [{ filePath: "<ROOT>/home/.mode/commands/a.md", enabled: false }],
  },
];

const tempRoots: string[] = [];

function tempRoot(label: string): string {
  const root = mkdtempSync(path.join(tmpdir(), `mode-commands-fixture-${label}-`));
  tempRoots.push(root);
  return root;
}

/**
 * 把 `<ROOT>` 占位替换为实际根（递归处理字符串与键）。
 *
 * 含占位的字符串一律按**路径模板**处理：`<ROOT>/a/b` 用 `path.join` 重建，
 * 保证 Windows 上也得到反斜杠，和 `join()` 产出、override 键、配置里的路径一致。
 */
function materialize<T>(value: T, root: string): T {
  if (typeof value === "string") {
    const index = value.indexOf(ROOT_TOKEN);
    if (index === -1) return value;
    const suffix = value
      .slice(index + ROOT_TOKEN.length)
      .replace(/^[\\/]+/, "");
    const absolute = suffix
      ? path.join(root, ...suffix.split(/[\\/]+/))
      : root;
    return `${value.slice(0, index)}${absolute}` as unknown as T;
  }
  if (Array.isArray(value)) {
    return value.map((item) => materialize(item, root)) as unknown as T;
  }
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [
        materialize(key, root),
        materialize(item, root),
      ]),
    ) as unknown as T;
  }
  return value;
}

/** 结果里的绝对路径归一成 `<ROOT>` + `/` 分隔，便于跨平台对拍。 */
function normalize(value: Json, root: string): Json {
  if (typeof value === "string") {
    return value.split(root).join(ROOT_TOKEN).split("\\").join("/");
  }
  if (Array.isArray(value)) {
    return value.map((item) => normalize(item, root));
  }
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [
        normalize(key, root) as string,
        normalize(item, root),
      ]),
    );
  }
  return value;
}

function writeFile(root: string, relativePath: string, content: string) {
  const target = path.join(root, ...relativePath.split("/"));
  mkdirSync(path.dirname(target), { recursive: true });
  writeFileSync(target, content);
}

function withHome<T>(home: string, run: () => Promise<T>): Promise<T> {
  const previousHome = process.env.HOME;
  const previousProfile = process.env.USERPROFILE;
  process.env.HOME = home;
  process.env.USERPROFILE = home;
  return run().finally(() => {
    if (previousHome === undefined) delete process.env.HOME;
    else process.env.HOME = previousHome;
    if (previousProfile === undefined) delete process.env.USERPROFILE;
    else process.env.USERPROFILE = previousProfile;
  });
}

try {
  const list = [];
  for (const [index, scenario] of listScenarios.entries()) {
    const root = tempRoot(`list-${index}`);
    const home = path.join(root, "home");
    mkdirSync(home, { recursive: true });
    for (const dir of scenario.dirs ?? []) {
      mkdirSync(path.join(root, ...dir.split("/")), { recursive: true });
    }
    for (const [relativePath, content] of scenario.files ?? []) {
      writeFile(root, relativePath, content);
    }
    for (const [relativePath, value] of scenario.jsonFiles ?? []) {
      writeFile(root, relativePath, JSON.stringify(materialize(value, root)));
    }
    if (scenario.config) {
      writeFile(
        root,
        "home/.mode/cli/config.json",
        `${JSON.stringify(materialize(scenario.config, root), null, 2)}\n`,
      );
    }

    const service = createCommandsService();
    const params: { agentSource?: "modeAgent"; workspacePath?: string } = {};
    if (scenario.listParams?.agentSource) params.agentSource = scenario.listParams.agentSource;
    if (scenario.listParams?.workspacePath) {
      params.workspacePath = path.join(root, ...scenario.listParams.workspacePath.split("/"));
    }
    const result = await withHome(home, () => service.list(params));

    list.push({
      id: scenario.id,
      dirs: scenario.dirs ?? [],
      files: scenario.files ?? [],
      jsonFiles: scenario.jsonFiles ?? [],
      config: scenario.config ?? null,
      listParams: scenario.listParams ?? {},
      expected: normalize(result, root),
    });
  }

  const override = [];
  for (const [index, scenario] of overrideCases.entries()) {
    const root = tempRoot(`override-${index}`);
    const home = path.join(root, "home");
    mkdirSync(home, { recursive: true });
    if (scenario.initial) {
      writeFile(
        root,
        "home/.mode/cli/config.json",
        `${JSON.stringify(materialize(scenario.initial, root), null, 2)}\n`,
      );
    }

    await withHome(home, async () => {
      const service = createCommandsService();
      for (const call of scenario.calls) {
        await service.setCommandEnabled({
          commandId: "ignored",
          filePath: materialize(call.filePath, root),
          enabled: call.enabled,
        });
      }
    });

    const raw = await readFile(path.join(home, ".mode/cli/config.json"), "utf-8");

    override.push({
      id: scenario.id,
      initial: scenario.initial ?? {},
      calls: scenario.calls,
      expected: normalize(JSON.parse(raw), root),
    });
  }

  const fixtures = {
    generatedBy: "scripts/generate-commands-fixtures.ts",
    rootToken: ROOT_TOKEN,
    parser: { cases: parserCases },
    generator: { cases: generatorCases },
    fileNames: { cases: fileNameCases },
    list: { scenarios: list },
    override: { cases: override },
  };

  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(OUTPUT, `${JSON.stringify(fixtures, null, 2)}\n`);
  console.log(
    `[commands] parser=${parserCases.length} generator=${generatorCases.length} ` +
      `fileNames=${fileNameCases.length} list=${list.length} override=${override.length} ` +
      `→ ${path.relative(ROOT_DIR, OUTPUT)}`,
  );
} finally {
  for (const root of tempRoots) rmSync(root, { recursive: true, force: true });
}
