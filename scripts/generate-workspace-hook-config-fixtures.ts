/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明目录树与期望输出，拆分会降低可核对性。 */
/**
 * 生成 `workspace-hook-config` 发现层的 Rust 对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/shared/src/workspace-hook-config.ts`
 *
 * ```bash
 * pnpm workspace-hook-config:fixtures
 * cmd //c "D:\\tools\\cargo-msvc.cmd test -p workspace-hook-config"
 * ```
 *
 * 路径策略：每个场景在临时目录里铺一棵树，跑完把绝对根替换成 `<root>` 再落盘；
 * Rust 侧用同一份树在它自己的临时根下重放，做同样的归一化后逐字段比较。
 */
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  createWorkspaceHookSourceInput,
  discoverWorkspaceHookConfigPaths,
  readWorkspaceHookProjectSources,
  resolveWorkspaceHookRuntimeRoot,
} from "../packages/shared/src/workspace-hook-config.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT_DIR, "crates", "workspace-hook-config", "fixtures", "hooks-config.json");

interface TreeEntry {
  path: string;
  content?: string;
}

interface Scenario {
  name: string;
  tree: TreeEntry[];
  cwd: string;
  explicit?: string;
}

// 注意：配置文件顶层是 `{ hooks: … }`，不是 hooks 本身；少了外层键会被读取逻辑当成「无 hooks」跳过。
const VALID_HOOKS = JSON.stringify(
  {
    hooks: {
      enabled: true,
      timeoutMs: 45_000,
      maxOutputBytes: 1_024,
      events: {
        PreToolUse: [
          {
            matcher: "Bash",
            hooks: [{ type: "command", command: "echo pre", shell: true, extraPassthrough: 1 }],
          },
        ],
        Stop: [{ hooks: [{ type: "process", command: "node", args: ["stop.js"] }] }],
      },
    },
  },
  null,
  2,
);

const SCENARIOS: Scenario[] = [
  {
    name: "worktree-ancestors",
    cwd: "repo/src/deep",
    tree: [
      { path: "repo/.git" },
      { path: "repo/mode.json", content: VALID_HOOKS },
      { path: "repo/.mode/config.json", content: VALID_HOOKS },
      { path: "repo/src/mode.json", content: JSON.stringify({ hooks: { enabled: false } }) },
      { path: "repo/src/deep/.mode/config.json", content: JSON.stringify({ hooks: {} }) },
    ],
  },
  {
    name: "no-worktree-marker",
    cwd: "standalone/nested",
    tree: [
      { path: "standalone/mode.json", content: VALID_HOOKS },
      { path: "standalone/nested/.mode/config.json", content: JSON.stringify({ hooks: {} }) },
    ],
  },
  {
    name: "git-marker-is-file",
    cwd: "worktree/sub",
    tree: [
      { path: "worktree/.git", content: "gitdir: ../.git/worktrees/wt\n" },
      { path: "worktree/mode.json", content: VALID_HOOKS },
    ],
  },
  {
    name: "explicit-duplicates-auto-discovered",
    cwd: "repo",
    explicit: "repo/.mode/config.json",
    tree: [
      { path: "repo/.git" },
      { path: "repo/.mode/config.json", content: VALID_HOOKS },
    ],
  },
  {
    name: "explicit-outside-scan",
    cwd: "repo",
    explicit: "config/custom.json",
    tree: [
      { path: "repo/mode.json", content: VALID_HOOKS },
      { path: "config/custom.json", content: JSON.stringify({ hooks: { enabled: false } }) },
    ],
  },
  {
    name: "read-errors-and-skips",
    cwd: "repo",
    tree: [
      { path: "repo/.git" },
      { path: "repo/mode.json", content: "{ not json" },
      { path: "repo/.mode/config.json", content: JSON.stringify({ hooks: { unknownKey: 1 } }) },
      { path: "repo/.mode/extra.json" },
    ],
  },
  {
    name: "schema-edge-cases",
    cwd: "repo",
    tree: [
      { path: "repo/.git" },
      {
        path: "repo/mode.json",
        content: JSON.stringify({ hooks: { events: { UnknownEvent: [] } } }),
      },
      {
        path: "repo/.mode/config.json",
        content: JSON.stringify({
          hooks: { events: { Stop: [{ hooks: [] }] } },
        }),
      },
    ],
  },
  {
    name: "non-object-and-missing-hooks",
    cwd: "repo",
    tree: [
      { path: "repo/.git" },
      { path: "repo/mode.json", content: "[1,2,3]" },
      { path: "repo/.mode/config.json", content: JSON.stringify({ other: true }) },
      { path: "repo/src/mode.json", content: JSON.stringify({ hooks: null }) },
      { path: "repo/src/.mode/config.json", content: JSON.stringify({ hooks: { timeoutMs: -5 } }) },
    ],
  },
  {
    name: "invalid-hooks-null",
    cwd: "repo",
    tree: [{ path: "repo/mode.json", content: JSON.stringify({ hooks: null }) }],
  },
  {
    name: "invalid-negative-timeout",
    cwd: "repo",
    tree: [{ path: "repo/.mode/config.json", content: JSON.stringify({ hooks: { timeoutMs: -5 } }) }],
  },
  {
    name: "invalid-hook-type",
    cwd: "repo",
    tree: [
      {
        path: "repo/mode.json",
        content: JSON.stringify({ hooks: { events: { Stop: [{ hooks: [{ type: "script", command: "x" }] }] } } }),
      },
    ],
  },
  {
    name: "invalid-missing-command",
    cwd: "repo",
    tree: [
      {
        path: "repo/mode.json",
        content: JSON.stringify({ hooks: { events: { Stop: [{ hooks: [{ type: "command" }] }] } } }),
      },
    ],
  },
  {
    name: "invalid-shell-false",
    cwd: "repo",
    tree: [
      {
        path: "repo/mode.json",
        content: JSON.stringify({
          hooks: { events: { Stop: [{ hooks: [{ type: "command", command: "x", shell: false }] }] } },
        }),
      },
    ],
  },
  {
    name: "invalid-events-array",
    cwd: "repo",
    tree: [{ path: "repo/mode.json", content: JSON.stringify({ hooks: { events: [] } }) }],
  },
  {
    name: "invalid-matcher-extra-key",
    cwd: "repo",
    tree: [
      {
        path: "repo/mode.json",
        content: JSON.stringify({
          hooks: { events: { Stop: [{ matcher: "x", extra: 1, hooks: [{ type: "process", command: "y" }] }] } },
        }),
      },
    ],
  },
  {
    name: "passthrough-unknown-fields",
    cwd: "repo",
    tree: [
      {
        path: "repo/.mode/config.json",
        content: JSON.stringify({
          hooks: {
            events: {
              Stop: [
                {
                  hooks: [
                    { type: "process", command: "y", args: ["a"], unknownField: { nested: [1, 2] } },
                  ],
                },
              ],
            },
          },
        }),
      },
    ],
  },
  {
    name: "directory-named-mode-json",
    cwd: "repo",
    tree: [{ path: "repo/.git" }, { path: "repo/mode.json" }],
  },
  {
    name: "mode-json-and-editable",
    cwd: "repo",
    tree: [
      { path: "repo/.git" },
      { path: "repo/mode.json", content: VALID_HOOKS },
      { path: "repo/.mode/config.json", content: JSON.stringify({ hooks: { enabled: true } }) },
      { path: "repo/pkg/mode.json", content: JSON.stringify({ hooks: { enabled: true } }) },
    ],
  },
];

function normalizePathFactory(root: string): (value: string) => string {
  return (value: string) =>
    value.replaceAll(root, "<root>").split(path.sep).join("/");
}

function classifyError(error: unknown): "syntax" | "schema" | "io" {
  if (error instanceof SyntaxError) return "syntax";
  if (typeof error === "object" && error !== null && "issues" in error) return "schema";
  return "io";
}

function materialize(root: string, tree: TreeEntry[]): void {
  for (const entry of tree) {
    const target = path.join(root, entry.path);
    if (entry.content === undefined) {
      mkdirSync(target, { recursive: true });
    } else {
      mkdirSync(path.dirname(target), { recursive: true });
      writeFileSync(target, entry.content);
    }
  }
}

async function runScenario(scenario: Scenario) {
  const root = mkdtempSync(path.join(tmpdir(), "zc-hooks-config-"));
  try {
    materialize(root, scenario.tree);
    const normalize = normalizePathFactory(root);
    const input = {
      workingDirectory: path.join(root, scenario.cwd),
      ...(scenario.explicit ? { explicitProjectConfigPath: path.join(root, scenario.explicit) } : {}),
    };
    const refs = discoverWorkspaceHookConfigPaths(input);
    const { sources, errors } = await readWorkspaceHookProjectSources(input);
    return {
      name: scenario.name,
      tree: scenario.tree,
      cwd: scenario.cwd,
      explicit: scenario.explicit ?? null,
      refs: refs.map((ref) => ({
        path: normalize(ref.path),
        explicitProjectConfig: ref.explicitProjectConfig,
      })),
      sources: sources.map((source) => ({
        canonicalPath: normalize(source.canonicalPath),
        baseDir: normalize(source.baseDir),
        discoveryOrder: source.discoveryOrder,
        configFileKind: source.configFileKind,
        explicitProjectConfig: source.explicitProjectConfig,
        editable: source.editable,
        hooks: source.hooks,
      })),
      errors: errors.map((entry) => ({
        path: normalize(entry.path),
        category: classifyError(entry.error),
      })),
    };
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

function runtimeRootCases() {
  const inputs: Array<Array<Record<string, unknown> | null>> = [
    [],
    [null],
    [{ enabled: true }],
    [{ enabled: false }, { enabled: true }],
    [{ timeoutMs: 1_000.6 }, { timeoutMs: 2_000 }],
    [null, { maxOutputBytes: 0.4 }, { maxOutputBytes: 1_024 }],
    [{ enabled: true, timeoutMs: -3, maxOutputBytes: -1 }],
    [{ enabled: true }, null, { timeoutMs: 0.5 }],
  ];
  return inputs.map((roots) => ({
    roots,
    expected: resolveWorkspaceHookRuntimeRoot(roots as never),
  }));
}

function sourceInputCases() {
  const root = path.join(tmpdir(), "zc-hooks-source-shape");
  const normalize = normalizePathFactory(root);
  const cases = [
    { path: path.join(root, "ws", ".mode", "config.json"), cwd: path.join(root, "ws"), explicit: false },
    { path: path.join(root, "ws", "mode.json"), cwd: path.join(root, "ws"), explicit: false },
    { path: path.join(root, "ws", "mode.json"), cwd: path.join(root, "ws"), explicit: true },
    { path: path.join(root, "other", ".mode", "config.json"), cwd: path.join(root, "ws"), explicit: true },
    { path: path.join(root, "other", ".mode", "config.json"), cwd: path.join(root, "ws"), explicit: false },
  ];
  return cases.map((entry, index) => {
    const source = createWorkspaceHookSourceInput({
      path: entry.path,
      workingDirectory: entry.cwd,
      hooks: index === 0 ? { events: {} } : {},
      discoveryOrder: index,
      explicitProjectConfig: entry.explicit,
    });
    return {
      input: { path: normalize(entry.path), cwd: normalize(entry.cwd), explicit: entry.explicit },
      expected: {
        canonicalPath: normalize(source.canonicalPath),
        baseDir: normalize(source.baseDir),
        discoveryOrder: source.discoveryOrder,
        configFileKind: source.configFileKind,
        explicitProjectConfig: source.explicitProjectConfig,
        editable: source.editable,
        hooks: source.hooks,
      },
    };
  });
}

async function main(): Promise<void> {
  const scenarios = [];
  for (const scenario of SCENARIOS) {
    scenarios.push(await runScenario(scenario));
  }
  const fixture = {
    scenarios,
    runtimeRoots: runtimeRootCases(),
    sourceInputs: sourceInputCases(),
  };
  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(OUTPUT, `${JSON.stringify(fixture, null, 2)}\n`);
  process.stdout.write(`wrote ${path.relative(ROOT_DIR, OUTPUT)} (${scenarios.length} scenarios)\n`);
}

await main();
