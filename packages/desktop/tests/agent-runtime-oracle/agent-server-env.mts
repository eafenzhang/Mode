// C1 Task 10：Rust `agent-server` 回放的环境准备（二进制定位/构建 + 已迁移基线库）。
//
// 拆出本文件是为了守住 oxlint 的 `max-lines`（400 行，错误级），不是第二套纪律。
//
// - 二进制：`MODE_AGENT_SERVER_COMMAND` 优先，否则 `target/{debug,release}/{agent-server,
//   mode-agent}`；缺失时自动跑一次 `cargo build -p agent-server`（前置条件，文档化）。
// - 基线库：C1 存储门是 fail-closed 的 schema 门（不建库、不迁移），回放/探针必须在
//   启动前把 `{base}/.mode/cli/db/db.sqlite` 预建成**已迁移**形状。DDL 从
//   `scripts/sqlite-schema-baseline.json` 生成，与 `crates/agent-server/tests/runtime.rs`
//   的同一实现（跨语言不能直接引用，改字段必须两边同步）。
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";
import { mkdir, readFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

export const REPO_ROOT = fileURLToPath(new URL("../../../../", import.meta.url));
const SCHEMA_BASELINE = join(REPO_ROOT, "scripts", "sqlite-schema-baseline.json");

export function resolveAgentServerBinary(): string | undefined {
  const configured = process.env.MODE_AGENT_SERVER_COMMAND?.trim();
  if (configured) return configured;
  for (const name of ["agent-server", "mode-agent"]) {
    for (const profile of ["debug", "release"]) {
      for (const suffix of [".exe", ""]) {
        const candidate = join(REPO_ROOT, "target", profile, `${name}${suffix}`);
        if (existsSync(candidate)) return candidate;
      }
    }
  }
  return undefined;
}

/**
 * 前置条件（文档化）：二进制缺失时跑一次 `cargo build -p agent-server` 再重试。
 * 构建失败/仍缺失 → 显式报错（与 Task 0 探针「binary 不存在即红」同一纪律）。
 */
export function ensureAgentServerBinary(): string {
  const existing = resolveAgentServerBinary();
  if (existing) return existing;
  const build = spawnSync("cargo", ["build", "-p", "agent-server"], {
    cwd: REPO_ROOT,
    encoding: "utf8",
    timeout: 600_000,
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (build.error || build.status !== 0) {
    throw new Error(
      `agent-server 二进制不存在且 cargo build 失败（status=${build.status}）：` +
        `${build.stderr ?? build.error ?? ""}`,
    );
  }
  const built = resolveAgentServerBinary();
  if (!built) {
    throw new Error("cargo build -p agent-server 成功但仍未找到二进制（target/{debug,release}）");
  }
  return built;
}

const LEDGER_DDL =
  "CREATE TABLE IF NOT EXISTS schema_migration (" +
  "id text primary key, checksum text not null, app_version text, time_applied integer not null)";

/**
 * 预建 C1 存储门需要的**已迁移**会话库（`{base}/.mode/cli/db/db.sqlite`）。
 * 账本行与 `crates/agent-server/tests/runtime.rs::create_baseline_database` 同一实现。
 */
export async function createBaselineDatabase(baseDir: string): Promise<string> {
  const dbDir = join(baseDir, ".mode", "cli", "db");
  await mkdir(dbDir, { recursive: true });
  const dbPath = join(dbDir, "db.sqlite");
  const baseline = JSON.parse(await readFile(SCHEMA_BASELINE, "utf8")) as {
    databases: Record<string, { tables: Record<string, TableSpec> }>;
  };
  const tables = baseline.databases["session-store"]?.tables;
  if (!tables) {
    throw new Error(`schema 基线缺少 databases["session-store"].tables：${SCHEMA_BASELINE}`);
  }
  const db = new DatabaseSync(dbPath);
  try {
    for (const [name, table] of Object.entries(tables)) {
      db.exec(tableDdl(name, table));
    }
    for (const table of Object.values(tables)) {
      for (const index of table.indexes ?? []) {
        db.exec(index.sql);
      }
    }
    db.exec(LEDGER_DDL);
    db.prepare(
      "INSERT INTO schema_migration (id, checksum, app_version, time_applied) " +
        "VALUES ('0001_fixture', 'fixture-checksum', '0.0.0', 1)",
    ).run();
  } finally {
    db.close();
  }
  return dbPath;
}

type TableSpec = {
  columns: { name: string; type: string; notNull?: boolean; defaultValue?: string; primaryKey?: number }[];
  indexes?: { sql: string }[];
};

function tableDdl(name: string, table: TableSpec): string {
  const definitions: string[] = [];
  const primaryKeys: { position: number; name: string }[] = [];
  for (const column of table.columns) {
    let definition = `${quoteIdentifier(column.name)} ${column.type}`;
    if (column.notNull) definition += " NOT NULL";
    if (typeof column.defaultValue === "string") {
      definition += ` DEFAULT ${column.defaultValue}`;
    }
    definitions.push(definition);
    const position = column.primaryKey ?? 0;
    if (position > 0) primaryKeys.push({ position, name: column.name });
  }
  if (primaryKeys.length > 0) {
    primaryKeys.sort((left, right) => left.position - right.position);
    definitions.push(`PRIMARY KEY (${primaryKeys.map((key) => quoteIdentifier(key.name)).join(", ")})`);
  }
  return `CREATE TABLE ${quoteIdentifier(name)} (\n  ${definitions.join(",\n  ")}\n)`;
}

function quoteIdentifier(name: string): string {
  return `"${name.replaceAll('"', '""')}"`;
}
