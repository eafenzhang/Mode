/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明保存路径用例，拆分会降低可核对性。 */
/**
 * 生成 `workspace-hooks-service`（保存侧）的 Rust 对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/hooks/workspaceHookSettingsModel.ts`（写侧投影）
 * - `packages/services/src/hooks/hooksService.ts`（`saveHooks` / `writeModeHooksConfig`）
 * - `packages/shared/src/workspace-hook-mutation.ts`（原子写与已审核开关）
 *
 * ```bash
 * pnpm hooks-save:fixtures
 * cmd //c "D:\\tools\\cargo-msvc.cmd test -p workspace-hooks-service"
 * ```
 *
 * 路径策略：`saveHooks` 会写 `~/.mode/cli/config.json`，脚本把 HOME/USERPROFILE 指到临时目录；
 * 与工作区路径一样，落盘前把绝对根替换成 `<root>`，Rust 侧用自己的临时根还原。
 */
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  resolveNextRootEnabled,
  toModeHooksEvents,
} from "../packages/services/src/hooks/workspaceHookSettingsModel.js";
import { createHooksService } from "../packages/services/src/hooks/hooksService.js";
import {
  buildWorkspaceHookBundleSnapshot,
  type WorkspaceHookSourceInput,
} from "../packages/shared/src/workspace-hook-digest.js";
import { writeWorkspaceHookConfiguredToggle } from "../packages/shared/src/workspace-hook-mutation.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT_DIR, "crates", "workspace-hooks-service", "fixtures", "hooks-save.json");

interface FileSpec {
  path: string;
  content: string;
}

function normalizeWith(root: string, value: unknown): unknown {
  if (typeof value === "string") {
    // 路径可能已是正斜杠（<root> 占位符替换后），两种写法都要归一。
    const forward = root.split(path.sep).join("/");
    let replaced = root.length > 0 ? value.replaceAll(root, "<root>") : value;
    if (forward.length > 0) replaced = replaced.replaceAll(forward, "<root>");
    return replaced.split(path.sep).join("/");
  }
  if (Array.isArray(value)) return value.map((item) => normalizeWith(root, item));
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [key, child] of Object.entries(value as Record<string, unknown>)) {
      out[key] = normalizeWith(root, child);
    }
    return out;
  }
  return value;
}

function materialize(root: string, files: FileSpec[]): void {
  for (const file of files) {
    const target = path.join(root, file.path);
    mkdirSync(path.dirname(target), { recursive: true });
    writeFileSync(target, file.content);
  }
}

function readJson(filePath: string): unknown {
  try {
    return JSON.parse(readFileSync(filePath, "utf8")) as unknown;
  } catch {
    return null;
  }
}

const USER_HOOK = {
  id: "user-1",
  event: "SessionStart",
  type: "command",
  command: "echo user",
  enabled: true,
  editable: true,
  location: { source: "mode", scope: "user", directoryPath: "<home>/.mode/cli" },
};

const PROJECT_HOOK = {
  id: "proj-1",
  event: "PreToolUse",
  matcher: "Bash",
  type: "command",
  command: "echo proj",
  enabled: false,
  editable: true,
  timeout: 2,
  custom: { extra: "kept" },
  location: { source: "mode", scope: "project", directoryPath: "<root>/ws/.mode", projectPath: "<root>/ws" },
  configuredState: {
    sourceRootEnabled: true,
    declarationEnabled: true,
    runtimeHooksEnabled: true,
    configuredEnabled: true,
    sourcePath: "<root>/ws/.mode/config.json",
  },
};

const PROJECT_PROCESS_HOOK = {
  id: "proj-2",
  event: "PreToolUse",
  matcher: "Bash",
  type: "process",
  command: "node",
  args: ["a.js"],
  async: true,
  shell: "bash",
  timeout: 1.5,
  enabled: true,
  editable: true,
  location: { source: "mode", scope: "project", directoryPath: "<root>/ws/.mode", projectPath: "<root>/ws" },
  configuredState: {
    sourceRootEnabled: true,
    declarationEnabled: false,
    runtimeHooksEnabled: true,
    configuredEnabled: true,
    sourcePath: "<root>/ws/.mode/config.json",
  },
};

const OTHER_PROJECT_HOOK = {
  id: "other-1",
  event: "Stop",
  type: "command",
  command: "echo other",
  enabled: true,
  editable: true,
  location: { source: "mode", scope: "project", directoryPath: "<root>/other/.mode", projectPath: "<root>/other" },
  configuredState: {
    sourceRootEnabled: true,
    declarationEnabled: true,
    runtimeHooksEnabled: true,
    configuredEnabled: true,
    sourcePath: "<root>/other/.mode/config.json",
  },
};

const LOCKED_HOOK = {
  id: "locked-1",
  event: "Stop",
  type: "command",
  command: "echo locked",
  enabled: true,
  editable: false,
  location: { source: "mode", scope: "project", directoryPath: "<root>/ws/.mode", projectPath: "<root>/ws" },
};

const SAVE_SCENARIOS = [
  {
    name: "mixed-sources",
    files: [
      {
        path: "ws/.mode/config.json",
        content: JSON.stringify(
          { hooks: { enabled: false, events: { Stop: [{ hooks: [{ type: "command", command: "old" }] }] } }, other: 1 },
          null,
          2,
        ),
      },
      {
        path: "home/.mode/cli/config.json",
        content: JSON.stringify({ hooks: { enabled: false, timeoutMs: 5000 } }, null, 2),
      },
    ],
    hooks: [USER_HOOK, PROJECT_HOOK, PROJECT_PROCESS_HOOK, OTHER_PROJECT_HOOK, LOCKED_HOOK],
    expect: ["ws/.mode/config.json", "home/.mode/cli/config.json"],
  },
  {
    name: "creates-files",
    files: [],
    hooks: [USER_HOOK, PROJECT_HOOK],
    expect: ["ws/.mode/config.json", "home/.mode/cli/config.json"],
  },
  {
    name: "keeps-existing-root-enabled",
    files: [
      {
        path: "ws/.mode/config.json",
        content: JSON.stringify({ hooks: { enabled: true } }, null, 2),
      },
    ],
    hooks: [{ ...PROJECT_HOOK, enabled: true }],
    expect: ["ws/.mode/config.json", "home/.mode/cli/config.json"],
  },
];

async function saveScenarios() {
  const results = [];
  for (const scenario of SAVE_SCENARIOS) {
    const root = mkdtempSync(path.join(tmpdir(), "zc-hooks-save-"));
    const home = path.join(root, "home");
    const previousHome = process.env.HOME;
    const previousProfile = process.env.USERPROFILE;
    try {
      materialize(root, scenario.files);
      process.env.HOME = home;
      process.env.USERPROFILE = home;
      // 输入里的 <root>/<home> 占位符换成真实路径，供 TS 侧执行。
      const concrete = JSON.parse(
        JSON.stringify(scenario.hooks)
          .replaceAll("<root>", root.split(path.sep).join("/"))
          .replaceAll("<home>", home.split(path.sep).join("/")),
      ) as unknown[];
      const service = createHooksService();
      await service.saveHooks({ workspacePath: path.join(root, "ws"), hooks: concrete as never });
      const files = Object.fromEntries(
        scenario.expect.map((relative) => [relative, readJson(path.join(root, relative))]),
      ) as Record<string, unknown>;
      results.push({
        name: scenario.name,
        files: scenario.files,
        hooks: normalizeWith(root, concrete),
        expect: scenario.expect,
        result: normalizeWith(root, files),
      });
    } finally {
      if (previousHome === undefined) delete process.env.HOME;
      else process.env.HOME = previousHome;
      if (previousProfile === undefined) delete process.env.USERPROFILE;
      else process.env.USERPROFILE = previousProfile;
      rmSync(root, { recursive: true, force: true });
    }
  }
  return results;
}

async function toggleScenarios() {
  const root = mkdtempSync(path.join(tmpdir(), "zc-hooks-toggle-"));
  try {
    const configPath = path.join(root, "ws", ".mode", "config.json");
    const source: WorkspaceHookSourceInput = {
      canonicalPath: configPath,
      baseDir: path.join(root, "ws"),
      discoveryOrder: 0,
      configFileKind: ".mode/config.json",
      explicitProjectConfig: false,
      editable: true,
      hooks: {
        events: {
          PreToolUse: [
            { matcher: "Bash", hooks: [{ type: "command", command: "echo a" }] },
            { hooks: [{ type: "process", command: "node", args: ["b.js"] }] },
          ],
        },
      } as never,
    };
    const snapshot = buildWorkspaceHookBundleSnapshot({
      workspaceIdentity: path.join(root, "ws"),
      workspacePath: path.join(root, "ws"),
      sources: [source],
      runtimeRoot: { enabled: true, timeoutMs: 60_000, maxOutputBytes: 32_768 },
      discoveredAt: "2026-10-30T00:00:00.000Z",
    });
    if (!snapshot) throw new Error("snapshot missing");
    const writeConfig = (extra: Record<string, unknown> = {}) => {
      mkdirSync(path.dirname(configPath), { recursive: true });
      writeFileSync(
        configPath,
        JSON.stringify({ hooks: source.hooks, other: "kept", ...extra }, null, 2),
      );
    };
    const reviewItemId = snapshot.hooks[0]!.reviewItemId;
    const cases: Array<Record<string, unknown>> = [];

    const runCase = async (name: string, mutate: () => void, target: string, enabled: boolean) => {
      writeConfig();
      mutate();
      const configBefore = readFileSync(configPath, "utf8");
      let error: { code: string; message: string } | null = null;
      try {
        await writeWorkspaceHookConfiguredToggle({
          configPath,
          snapshot,
          reviewItemId: target,
          enabled,
        });
      } catch (caught) {
        const value = caught as { code?: string; message?: string };
        error = { code: value.code ?? "unknown", message: value.message ?? String(caught) };
      }
      cases.push({
        name,
        configPath: "ws/.mode/config.json",
        snapshot,
        reviewItemId: target,
        enabled,
        configBefore,
        error,
        result: error ? null : readJson(configPath),
      });
    };

    await runCase("toggle-off", () => undefined, reviewItemId, false);
    await runCase("toggle-on-second", () => undefined, snapshot.hooks[1]!.reviewItemId, true);
    await runCase("unknown-id", () => undefined, "workspace-hook-missing", true);
    await runCase(
      "declaration-changed",
      () => {
        writeFileSync(
          configPath,
          JSON.stringify(
            {
              hooks: {
                events: {
                  PreToolUse: [
                    { matcher: "Bash", hooks: [{ type: "command", command: "echo changed" }] },
                  ],
                },
              },
            },
            null,
            2,
          ),
        );
      },
      reviewItemId,
      true,
    );
    await runCase(
      "unreadable",
      () => {
        writeFileSync(configPath, "{ not json");
      },
      reviewItemId,
      true,
    );

    return { scenarios: normalizeWith(root, cases) as Array<Record<string, unknown>>, snapshot: normalizeWith(root, snapshot) };
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

function pureCases() {
  const hooks = [USER_HOOK, PROJECT_HOOK, PROJECT_PROCESS_HOOK];
  return {
    events: [
      { hooks, expected: toModeHooksEvents(hooks as never) },
      {
        hooks: [
          { ...PROJECT_HOOK, matcher: undefined },
          { ...PROJECT_PROCESS_HOOK, matcher: undefined },
        ],
        expected: toModeHooksEvents(
          [
            { ...PROJECT_HOOK, matcher: undefined },
            { ...PROJECT_PROCESS_HOOK, matcher: undefined },
          ] as never,
        ),
      },
    ],
    rootEnabled: [
      { existing: undefined, hooks, expected: resolveNextRootEnabled(undefined, hooks as never) ?? null },
      { existing: false, hooks: [{ ...PROJECT_HOOK, enabled: true }], expected: resolveNextRootEnabled(false, [{ ...PROJECT_HOOK, enabled: true }] as never) ?? null },
      { existing: true, hooks: [PROJECT_HOOK], expected: resolveNextRootEnabled(true, [PROJECT_HOOK] as never) ?? null },
    ],
  };
}

async function main(): Promise<void> {
  const fixture = {
    pure: normalizeWith("", pureCases()),
    save: await saveScenarios(),
    toggle: await toggleScenarios(),
  };
  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(OUTPUT, `${JSON.stringify(fixture, null, 2)}\n`);
  process.stdout.write(`wrote ${path.relative(ROOT_DIR, OUTPUT)}\n`);
}

await main();
