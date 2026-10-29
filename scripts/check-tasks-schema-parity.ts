/**
 * 任务库 schema 生产者的实时对拍：TS `buildTasksSchemaSnapshot()` vs Rust 迁移。
 *
 * 背景：SQLite 门禁当前仍以 TS 迁移为权威（Node 仍写任务库）。Rust 侧已经复刻了
 * schema 与 3 条冻结迁移，但只有在有人改 TS 迁移时才会暴露漂移；本脚本在 CI 里把
 * 两个生产者都跑一遍并逐表比较，保证切换生产者之前两边始终一致。
 *
 * ```bash
 * pnpm check:tasks-schema-parity
 * ```
 *
 * 本地 cargo 不在 PATH 时用 `CARGO=D:\tools\cargo-msvc.cmd pnpm check:tasks-schema-parity`。
 */
import { execFileSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { canonicalSchemaJson } from "./lib/sqlite-schema.js";
import { buildTasksSchemaSnapshot } from "./sqlite-schema-snapshot.js";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

interface DatabaseSnapshot {
  tables: Record<string, unknown>;
}

function runRustSnapshot(): DatabaseSnapshot {
  const cargo = process.env.CARGO?.trim() || "cargo";
  const args = [
    "run",
    "--quiet",
    "--locked",
    "-p",
    "tasks-db",
    "--bin",
    "tasks-db-snapshot",
  ];
  // Windows 的 cargo wrapper 是 .cmd，execFile 不能直接启动批处理。
  const output = cargo.toLowerCase().endsWith(".cmd")
    ? execFileSync("cmd", ["/c", cargo, ...args], { cwd: ROOT, encoding: "utf8" })
    : execFileSync(cargo, args, { cwd: ROOT, encoding: "utf8" });
  return JSON.parse(output) as DatabaseSnapshot;
}

function describeMismatch(left: DatabaseSnapshot, right: DatabaseSnapshot): string {
  const leftTables = new Set(Object.keys(left.tables));
  const rightTables = new Set(Object.keys(right.tables));
  const missingInRust = [...leftTables].filter((name) => !rightTables.has(name));
  const extraInRust = [...rightTables].filter((name) => !leftTables.has(name));
  const changedTables = [...leftTables]
    .filter((name) => rightTables.has(name))
    .filter(
      (name) =>
        canonicalSchemaJson(left.tables[name]) !== canonicalSchemaJson(right.tables[name]),
    );

  const lines: string[] = [];
  if (missingInRust.length > 0) lines.push(`Rust 缺少表: ${missingInRust.join(", ")}`);
  if (extraInRust.length > 0) lines.push(`Rust 多出表: ${extraInRust.join(", ")}`);
  if (changedTables.length > 0) lines.push(`表定义不一致: ${changedTables.join(", ")}`);
  return lines.join("\n");
}

const tsSnapshot = buildTasksSchemaSnapshot();
const rustSnapshot = runRustSnapshot();
const tsJson = canonicalSchemaJson(tsSnapshot);
const rustJson = canonicalSchemaJson(rustSnapshot);

if (tsJson !== rustJson) {
  console.error("[tasks-schema] TS 与 Rust 的任务库 schema 不一致");
  const detail = describeMismatch(tsSnapshot, rustSnapshot);
  if (detail) console.error(detail);
  process.exit(1);
}

console.log(
  `[tasks-schema] producers agree: tables=${Object.keys(tsSnapshot.tables).length}`,
);
