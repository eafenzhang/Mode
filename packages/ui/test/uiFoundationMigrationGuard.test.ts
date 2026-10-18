/**
 * M4 迁移守卫：已切到 `ui-foundation/adapters` 的旧模块，不允许被组件/业务代码重新 import。
 * 旧文件本身可以互相引用（它们只作为待删除的历史实现保留）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import test from "node:test";

const SRC_ROOT = join(process.cwd(), "src");
const LEGACY_DIALOG_DIR = join(SRC_ROOT, "components", "ui");

const MIGRATED_MODULES = [
  "alert-dialog",
  "alert",
  "avatar",
  "badge",
  "button",
  "card",
  "chart",
  "checkbox",
  "collapsible",
  "code-viewer",
  "command",
  "context-menu",
  "dialog",
  "diff-viewer",
  "dropdown-menu",
  "flip-metric-value",
  "hover-card",
  "input-group",
  "input",
  "kbd",
  "label",
  "lightweight-diff-preview",
  "highlighted-lightweight-diff-preview",
  "pdf-viewer",
  "popover",
  "progress",
  "pptx-selection-action-bar",
  "pptx-preview-viewer",
  "resizable",
  "scroll-area",
  "scroll-fade-viewport",
  "select",
  "separator",
  "spinner",
  "switch",
  "tabs",
  "textarea",
  "toast",
  "tooltip",
];

function walk(directory: string): string[] {
  return readdirSync(directory).flatMap((name) => {
    const path = join(directory, name);
    if (statSync(path).isDirectory()) {
      return walk(path);
    }
    return /\.(ts|tsx)$/.test(name) ? [path] : [];
  });
}

test("迁移守卫：业务代码不再 import 已替换的 components/ui 模块", () => {
  const offending: string[] = [];
  for (const file of walk(SRC_ROOT)) {
    if (file.startsWith(LEGACY_DIALOG_DIR)) {
      continue;
    }
    const source = readFileSync(file, "utf8");
    for (const moduleName of MIGRATED_MODULES) {
      const needle = `components/ui/${moduleName}.js`;
      if (source.includes(needle)) {
        offending.push(`${file}: ${needle}`);
      }
    }
  }
  assert.deepEqual(offending, []);
});
