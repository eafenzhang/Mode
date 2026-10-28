// 插件 hook 来源布局回归（docs/specs/plugin-host.md §5.10）。
// 源码从 ../dist 导入，先 build：pnpm --filter @mode/adapters build。
import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import type { PluginDiagnostic } from "@mode/contracts";

import { listPluginHookEventNames, listPluginHookSources } from "../dist/plugins/hook-sources.js";
import type { LoadedPlugin } from "../dist/plugins/types.js";

const HOOKS = { PreToolUse: [{ matcher: "Bash", hooks: [] }] };
const HOOKS_FILE = { hooks: HOOKS };

function createPluginRoot(t: test.TestContext): string {
  const root = mkdtempSync(join(tmpdir(), "mode-plugin-hooks-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  return root;
}

function loadedPlugin(root: string, hooks?: unknown): LoadedPlugin {
  return {
    id: "compat-plugin",
    manifest: { name: "compat-plugin", ...(hooks === undefined ? {} : { hooks }) },
    manifestPath: join(root, "plugin.json"),
    marketplace: "local",
    rootPath: root,
    source: "inline",
  };
}

function readSources(root: string, diagnostics: PluginDiagnostic[] = [], hooks?: unknown) {
  return listPluginHookSources({ diagnostics, loaded: loadedPlugin(root, hooks) });
}

test("只有根 hooks.json 时按 Codex 布局加载", (t) => {
  const root = createPluginRoot(t);
  writeFileSync(join(root, "hooks.json"), JSON.stringify(HOOKS_FILE));

  const sources = readSources(root);
  assert.equal(sources.length, 1);
  assert.equal(sources[0]?.sourcePath, join(root, "hooks.json"));
  assert.equal(sources[0]?.wrapper, true);
  assert.deepEqual(listPluginHookEventNames({ diagnostics: [], loaded: loadedPlugin(root) }), [
    "PreToolUse",
  ]);
});

test("同时存在标准路径和根 hooks.json 时标准路径优先", (t) => {
  const root = createPluginRoot(t);
  mkdirSync(join(root, "hooks"), { recursive: true });
  writeFileSync(join(root, "hooks", "hooks.json"), JSON.stringify(HOOKS_FILE));
  writeFileSync(join(root, "hooks.json"), JSON.stringify({ hooks: { Stop: [] } }));

  const sources = readSources(root);
  assert.equal(sources.length, 1);
  assert.equal(sources[0]?.sourcePath, join(root, "hooks", "hooks.json"));
});

test("根 hooks.json 与 manifest.hooks 指向同一文件时按 realpath 去重", (t) => {
  const root = createPluginRoot(t);
  writeFileSync(join(root, "hooks.json"), JSON.stringify(HOOKS_FILE));
  const diagnostics: PluginDiagnostic[] = [];

  const sources = readSources(root, diagnostics, "hooks.json");
  assert.equal(sources.length, 1);
  assert.ok(
    diagnostics.some(
      (diagnostic) =>
        diagnostic.code === "plugin_hook_invalid" &&
        diagnostic.severity === "warning" &&
        diagnostic.message.includes("Duplicate"),
    ),
  );
});

test("根 hooks.json 缺少 hooks 包装时报 plugin_hook_invalid", (t) => {
  const root = createPluginRoot(t);
  writeFileSync(join(root, "hooks.json"), JSON.stringify(HOOKS));
  const diagnostics: PluginDiagnostic[] = [];

  const eventNames = listPluginHookEventNames({
    diagnostics,
    loaded: loadedPlugin(root),
  });
  assert.deepEqual(eventNames, []);
  assert.ok(
    diagnostics.some(
      (diagnostic) => diagnostic.code === "plugin_hook_invalid" && diagnostic.severity === "error",
    ),
  );
});
