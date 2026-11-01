#!/usr/bin/env node
/**
 * `IModeAgentService` 调用点证据（W4 S1 辅助）。
 *
 * 对覆盖清单里的每个方法，扫描 Renderer / Desktop / Services / CLI 的源码调用点，
 * 生成 `docs/specs/agent-method-callsites.md`，供人工把 `todo` 归类为
 * `rust` / `not-needed` / `blocked`。本脚本只读源码，不改变运行时。
 *
 * 用法：node scripts/agent-method-callsites.ts
 *
 * 口径：
 * - 只统计 `<name>(`，不区分属性调用与解构后调用；
 * - 排除 node_modules / dist / 构建产物与 interface 自身；
 * - 每个方法最多列 3 个 `file:line` 例子；
 * - 同名通用方法（如 `list`）会有噪音，脚本只提供证据，不做自动判定。
 */
import { readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const MANIFEST = join(ROOT, "docs/specs/agent-method-coverage.json");
const OUTPUT = join(ROOT, "docs/specs/agent-method-callsites.md");
const SOURCE_ROOTS = [
  "packages/ui/src",
  "packages/desktop/src",
  "packages/services/src",
  "apps/mode-cli/packages",
];
const EXCLUDED_PATH_SEGMENTS = [
  "node_modules",
  "/dist/",
  "/dist-types/",
  "/out/",
  "/.plan/",
];
const EXCLUDED_FILES = new Set([
  "packages/services/src/mode-agent/modeAgent.ts",
]);

function walk(dir, files) {
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) {
      walk(path, files);
    } else if (/\.(?:ts|tsx|mts|cts)$/u.test(path)) {
      files.push(path);
    }
  }
}

function isExcluded(path) {
  const normalized = relative(ROOT, path).split("\\").join("/");
  if (EXCLUDED_FILES.has(normalized)) return true;
  return EXCLUDED_PATH_SEGMENTS.some((segment) => normalized.includes(segment));
}

function main() {
  const manifest = JSON.parse(readFileSync(MANIFEST, "utf8"));
  const files = [];
  for (const root of SOURCE_ROOTS) {
    const absolute = join(ROOT, root);
    try {
      walk(absolute, files);
    } catch {
      // 目录不存在时跳过（不同分支可能没有某个包）。
    }
  }
  const sources = files
    .filter((path) => !isExcluded(path))
    .map((path) => ({ path, text: readFileSync(path, "utf8") }));

  const rows = [];
  for (const entry of manifest) {
    const pattern = new RegExp(`(?:\\.|\\b)${entry.method}\\s*\\(`, "u");
    const calls = [];
    for (const source of sources) {
      const lines = source.text.split("\n");
      lines.forEach((line, index) => {
        if (pattern.test(line)) {
          calls.push(`${relative(ROOT, source.path).split("\\").join("/")}:${index + 1}`);
        }
      });
    }
    rows.push({ ...entry, calls });
  }

  rows.sort((a, b) => a.method.localeCompare(b.method));
  const header = [
    "# IModeAgentService 调用点证据（生成物）",
    "",
    "由 `node scripts/agent-method-callsites.ts` 生成；不要手改。",
    "计数是启发式（同名方法可能混入噪音），只用于 S1 归类时提供证据。",
    "",
    "| 方法 | status | 调用点数 | 例子 |",
    "| --- | --- | ---: | --- |",
  ];
  const body = rows.map((row) => {
    const examples = row.calls.slice(0, 3).join("<br>");
    return `| \`${row.method}\` | ${row.status} | ${row.calls.length} | ${examples} |`;
  });
  writeFileSync(OUTPUT, `${[...header, ...body].join("\n")}\n`);
  const zero = rows.filter((row) => row.calls.length === 0).length;
  console.log(
    `agent-method-callsites: ${rows.length} 个方法（${zero} 个无调用点）→ ${relative(ROOT, OUTPUT)}`,
  );
}

main();
