#!/usr/bin/env node
/**
 * `IModeAgentService` 覆盖矩阵（W4 所有权切换 S1）。
 *
 * 真源：`packages/services/src/mode-agent/modeAgent.ts` 的 interface 方法名。
 * 清单：`docs/specs/agent-method-coverage.json` —— 每个方法一条记录：
 *   - status: "rust" | "todo" | "not-needed" | "blocked"
 *   - rust?: 覆盖它的 `crates/agent-process/src` 函数名（snake_case）
 *   - note?: 人工说明（`not-needed` / `blocked` 必须写）
 *
 * 用法：
 *   node scripts/agent-method-coverage.ts --write   # 重新检测 rust 覆盖并保留人工状态
 *   node scripts/agent-method-coverage.ts --check   # CI/提交前校验
 *
 * 校验项：
 *   1. 清单与 interface 方法集合完全一致（双向，防止漏记与陈旧条目）；
 *   2. status=rust 的函数确实存在于 crates/agent-process/src；
 *   3. not-needed / blocked 必须带 note；
 *   4. status 值合法。
 *
 * 本脚本只读源码，不改变运行时行为。
 */
import { readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const INTERFACE_FILE = join(ROOT, "packages/services/src/mode-agent/modeAgent.ts");
const RUST_DIR = join(ROOT, "crates/agent-process/src");
const MANIFEST_FILE = join(ROOT, "docs/specs/agent-method-coverage.json");

const VALID_STATUS = new Set(["rust", "todo", "not-needed", "blocked"]);

function parseInterfaceMethods(source) {
  const start = source.indexOf("export interface IModeAgentService {");
  if (start < 0) throw new Error("找不到 IModeAgentService");
  const end = source.indexOf("\n}", start);
  if (end < 0) throw new Error("找不到 IModeAgentService 结束位置");
  const block = source.slice(start, end);
  const methods = new Set();
  for (const line of block.split("\n")) {
    const match = /^ {2}([A-Za-z][A-Za-z0-9]*)\??\s*(?:<[^>]*>)?\s*\(/u.exec(line);
    if (match) methods.add(match[1]);
  }
  return [...methods].sort();
}

function toSnakeCase(name) {
  // 只在小写/数字 → 大写、连续大写 → 词首大写处插下划线；
  // `V4` 这类字母+数字后缀保持连写（attachmentBeginV4 → attachment_begin_v4）。
  return name
    .replace(/([a-z0-9])([A-Z])/gu, "$1_$2")
    .replace(/([A-Z]+)([A-Z][a-z])/gu, "$1_$2")
    .toLowerCase();
}

function readRustSources() {
  const files = [];
  const walk = (dir) => {
    for (const entry of readdirSync(dir)) {
      const path = join(dir, entry);
      if (statSync(path).isDirectory()) walk(path);
      else if (path.endsWith(".rs")) files.push(path);
    }
  };
  walk(RUST_DIR);
  return files.map((path) => readFileSync(path, "utf8")).join("\n");
}

function fnExists(rustSource, fnName) {
  return new RegExp(`fn\\s+${fnName}\\s*[<(]`, "u").test(rustSource);
}

function detectRustFn(rustSource, method) {
  const snake = toSnakeCase(method);
  return fnExists(rustSource, snake) ? snake : null;
}

function loadManifest() {
  return JSON.parse(readFileSync(MANIFEST_FILE, "utf8"));
}

function writeManifest(manifest) {
  const sorted = [...manifest].sort((a, b) => a.method.localeCompare(b.method));
  writeFileSync(MANIFEST_FILE, `${JSON.stringify(sorted, null, 2)}\n`);
}

function main() {
  const args = process.argv.slice(2);
  const mode = args.find((value) => value === "--write" || value === "--check");
  const strict = args.includes("--strict");
  if (mode !== "--write" && mode !== "--check") {
    console.error("用法：node scripts/agent-method-coverage.ts --write|--check [--strict]");
    process.exit(2);
  }

  const methods = parseInterfaceMethods(readFileSync(INTERFACE_FILE, "utf8"));
  const rustSource = readRustSources();
  const manifest = loadManifest();
  const byMethod = new Map(manifest.map((entry) => [entry.method, entry]));

  if (mode === "--write") {
    const next = methods.map((method) => {
      const existing = byMethod.get(method);
      // 人工声明的 rust 映射优先（方法名与 Rust fn 名不同，如 V4 查询），
      // 但仍要验证函数真实存在；失效则退回自动检测/todo。
      const declared =
        existing?.status === "rust" && existing.rust && fnExists(rustSource, existing.rust)
          ? existing.rust
          : null;
      const rustFn = declared ?? detectRustFn(rustSource, method);
      const status =
        existing && existing.status !== "rust" && existing.status !== "todo"
          ? existing.status
          : rustFn
            ? "rust"
            : "todo";
      return {
        method,
        status,
        ...(rustFn && status === "rust" ? { rust: rustFn } : {}),
        ...(existing?.note ? { note: existing.note } : {}),
      };
    });
    writeManifest(next);
    report(next);
    return;
  }

  const problems = [];
  const methodSet = new Set(methods);
  for (const method of methods) {
    if (!byMethod.has(method)) problems.push(`缺少条目：${method}`);
  }
  for (const entry of manifest) {
    if (!methodSet.has(entry.method)) problems.push(`陈旧条目：${entry.method}`);
    if (!VALID_STATUS.has(entry.status)) {
      problems.push(`${entry.method}: 非法 status ${entry.status}`);
      continue;
    }
    if (entry.status === "rust") {
      if (!entry.rust) problems.push(`${entry.method}: status=rust 缺少 rust 函数名`);
      else if (!new RegExp(`fn\\s+${entry.rust}\\s*[<(]`, "u").test(rustSource)) {
        problems.push(`${entry.method}: rust 函数不存在（${entry.rust}）`);
      }
    }
    if ((entry.status === "not-needed" || entry.status === "blocked") && !entry.note) {
      problems.push(`${entry.method}: ${entry.status} 必须写 note`);
    }
  }

  if (strict) {
    for (const entry of manifest) {
      if (entry.status === "todo") problems.push(`未归类：${entry.method}`);
    }
  }

  report(manifest);
  if (problems.length > 0) {
    for (const problem of problems) console.error(`- ${problem}`);
    process.exit(1);
  }
  console.log("agent-method-coverage: OK");
}

function report(entries) {
  const counts = { rust: 0, todo: 0, "not-needed": 0, blocked: 0 };
  for (const entry of entries) counts[entry.status] = (counts[entry.status] ?? 0) + 1;
  console.log(
    `agent-method-coverage: ${entries.length} 个方法 · rust=${counts.rust} todo=${counts.todo} ` +
      `not-needed=${counts["not-needed"]} blocked=${counts.blocked}`,
  );
}

main();
