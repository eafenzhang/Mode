/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明配置矩阵，拆分会降低可核对性。 */
/**
 * 生成 `workspace-hook-digest` 条目解析与包级快照的 Rust 对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/shared/src/workspace-hook-digest.ts`（`resolveWorkspaceHookEntries` /
 *   `buildWorkspaceHookBundleSnapshot`）
 * - `packages/shared/src/workspace-hook-config.ts`（`SourceInput` 形状）
 *
 * ```bash
 * pnpm hook-snapshot:fixtures
 * cmd //c "D:\\tools\\cargo-msvc.cmd test -p workspace-hook-digest"
 * ```
 *
 * 全是纯函数输入，不落盘：路径用固定的 `D:/ws` 字面量，两端一致。
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  buildWorkspaceHookBundleSnapshot,
  resolveWorkspaceHookEntries,
  type WorkspaceHookBundleSnapshotData,
  type WorkspaceHookSourceInput,
} from "../packages/shared/src/workspace-hook-digest.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT_DIR, "crates", "workspace-hook-digest", "fixtures", "hook-snapshot.json");
const WORKSPACE = "D:/ws";
const DISCOVERED_AT = "2026-10-30T00:00:00.000Z";

interface Case {
  name: string;
  sources: WorkspaceHookSourceInput[];
  runtimeRoot: { enabled: boolean; timeoutMs: number; maxOutputBytes: number };
}

function source(input: {
  path: string;
  baseDir?: string;
  order: number;
  kind: "mode.json" | ".mode/config.json" | "explicit";
  explicit?: boolean;
  editable?: boolean;
  hooks: Record<string, unknown>;
}): WorkspaceHookSourceInput {
  return {
    canonicalPath: input.path,
    baseDir:
      input.baseDir ??
      (input.path.endsWith("/.mode/config.json")
        ? path.posix.dirname(path.posix.dirname(input.path))
        : path.posix.dirname(input.path)),
    discoveryOrder: input.order,
    configFileKind: input.kind,
    explicitProjectConfig: input.explicit ?? false,
    editable: input.editable ?? false,
    hooks: input.hooks as never,
  };
}

const DEFAULT_ROOT = { enabled: true, timeoutMs: 60_000, maxOutputBytes: 32_768 };

const CASES: Case[] = [
  {
    name: "basic",
    runtimeRoot: DEFAULT_ROOT,
    sources: [
      source({
        path: "D:/ws/.mode/config.json",
        order: 0,
        kind: ".mode/config.json",
        editable: true,
        hooks: {
          timeoutMs: 45_000,
          events: {
            PreToolUse: [
              { matcher: "Bash", hooks: [{ type: "command", command: "echo pre" }] },
              { hooks: [{ type: "process", command: "node", args: ["a.js", "b"] }] },
            ],
            Stop: [{ hooks: [{ type: "command", command: "echo stop", statusMessage: "收尾中" }] }],
          },
        },
      }),
    ],
  },
  {
    name: "source-disabled",
    runtimeRoot: DEFAULT_ROOT,
    sources: [
      source({
        path: "D:/ws/mode.json",
        order: 0,
        kind: "mode.json",
        hooks: {
          enabled: false,
          events: { Stop: [{ hooks: [{ type: "command", command: "echo stop" }] }] },
        },
      }),
    ],
  },
  {
    name: "hook-disabled-and-root-disabled",
    runtimeRoot: { enabled: false, timeoutMs: 60_000, maxOutputBytes: 32_768 },
    sources: [
      source({
        path: "D:/ws/.mode/config.json",
        order: 1,
        kind: ".mode/config.json",
        hooks: {
          enabled: true,
          events: {
            Stop: [
              { hooks: [{ type: "command", command: "echo a", enabled: false }] },
              { hooks: [{ type: "command", command: "echo b", enabled: true }] },
            ],
          },
        },
      }),
    ],
  },
  {
    name: "shell-and-async",
    runtimeRoot: DEFAULT_ROOT,
    sources: [
      source({
        path: "D:/ws/.mode/config.json",
        order: 2,
        kind: ".mode/config.json",
        hooks: {
          events: {
            PostToolUse: [
              {
                hooks: [
                  { type: "command", command: "one", shell: true },
                  { type: "command", command: "two", shell: "bash -lc" },
                  { type: "command", command: "three", async: true },
                  { type: "command", command: "four", async: false },
                  { type: "process", command: "five", args: [] },
                ],
              },
            ],
          },
        },
      }),
    ],
  },
  {
    name: "timeouts",
    runtimeRoot: { enabled: true, timeoutMs: 10_000, maxOutputBytes: 1_024 },
    sources: [
      source({
        path: "D:/ws/.mode/config.json",
        order: 0,
        kind: ".mode/config.json",
        hooks: {
          timeoutMs: 20_000,
          maxOutputBytes: 2_048,
          events: {
            SessionStart: [
              {
                hooks: [
                  { type: "command", command: "a", timeout: 2 },
                  { type: "command", command: "b", timeoutMs: 500 },
                  { type: "process", command: "c", timeout: 3 },
                  { type: "command", command: "d", timeoutMs: 0.4 },
                ],
              },
            ],
          },
        },
      }),
    ],
  },
  {
    name: "two-sources",
    runtimeRoot: DEFAULT_ROOT,
    sources: [
      source({
        path: "D:/ws/.mode/config.json",
        order: 0,
        kind: ".mode/config.json",
        editable: true,
        hooks: { events: { Stop: [{ hooks: [{ type: "command", command: "local" }] }] } },
      }),
      source({
        path: "D:/ws/.config/custom.json",
        baseDir: "D:/ws/.config",
        order: 1,
        kind: "explicit",
        explicit: true,
        hooks: { events: { Stop: [{ hooks: [{ type: "process", command: "explicit" }] }] } },
      }),
    ],
  },
  {
    name: "nested-relative-paths",
    runtimeRoot: DEFAULT_ROOT,
    sources: [
      source({
        path: "D:/ws/packages/app/.mode/config.json",
        baseDir: "D:/ws/packages/app",
        order: 0,
        kind: ".mode/config.json",
        hooks: { events: { UserPromptSubmit: [{ hooks: [{ type: "command", command: "x" }] }] } },
      }),
    ],
  },
  {
    name: "empty",
    runtimeRoot: DEFAULT_ROOT,
    sources: [
      source({
        path: "D:/ws/.mode/config.json",
        order: 0,
        kind: ".mode/config.json",
        hooks: {},
      }),
      source({
        path: "D:/ws/mode.json",
        order: 1,
        kind: "mode.json",
        hooks: { events: { Stop: [] } },
      }),
    ],
  },
];

function snapshotOrNull(
  input: Case,
): { entries: unknown[]; snapshot: WorkspaceHookBundleSnapshotData | null } {
  const entries = resolveWorkspaceHookEntries({
    workspacePath: WORKSPACE,
    sources: input.sources,
    runtimeRoot: input.runtimeRoot,
  });
  const snapshot =
    buildWorkspaceHookBundleSnapshot({
      workspaceIdentity: "D:/ws",
      workspacePath: WORKSPACE,
      sources: input.sources,
      runtimeRoot: input.runtimeRoot,
      discoveredAt: DISCOVERED_AT,
    }) ?? null;
  return { entries, snapshot };
}

function main(): void {
  const cases = CASES.map((input) => {
    const { entries, snapshot } = snapshotOrNull(input);
    return {
      name: input.name,
      workspacePath: WORKSPACE,
      workspaceIdentity: "D:/ws",
      discoveredAt: DISCOVERED_AT,
      sources: input.sources,
      runtimeRoot: input.runtimeRoot,
      entries,
      snapshot,
    };
  });
  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(OUTPUT, `${JSON.stringify({ cases }, null, 2)}\n`);
  process.stdout.write(`wrote ${path.relative(ROOT_DIR, OUTPUT)} (${cases.length} cases)\n`);
}

main();
