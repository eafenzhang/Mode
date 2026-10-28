import { DatabaseSync } from "node:sqlite";

import { SQLITE_MIGRATIONS } from "../apps/mode-cli/packages/adapters/src/storage/session-store/migrations.ts";
import { runTasksDatabaseMigrations } from "../packages/services/src/session/tasksDatabase/migrations.ts";
import { normalizeSchemaSql, SQLITE_SCHEMA_SNAPSHOT_VERSION } from "./lib/sqlite-schema.js";

const EXCLUDED_TABLES = new Set(["tasks_schema_migration", "schema_migration"]);

interface ColumnSnapshot {
  name: string;
  type: string;
  notNull: boolean;
  defaultValue: string | null;
  primaryKey: number;
}

interface IndexSnapshot {
  name: string;
  sql: string;
}

interface TableSnapshot {
  columns: ColumnSnapshot[];
  indexes: IndexSnapshot[];
}

interface DatabaseSnapshot {
  tables: Record<string, TableSnapshot>;
}

export interface SchemaSnapshot {
  schemaVersion: number;
  databases: Record<string, DatabaseSnapshot>;
}

function quoteIdentifier(name: string): string {
  return `"${name.replaceAll('"', '""')}"`;
}

function readDatabaseSchema(db: DatabaseSync): DatabaseSnapshot {
  const tableRows = db
    .prepare(
      "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .all() as Array<{ name: string }>;
  const tables: Record<string, TableSnapshot> = {};
  for (const { name } of tableRows) {
    if (EXCLUDED_TABLES.has(name)) continue;
    const columns = db
      .prepare(`PRAGMA table_info(${quoteIdentifier(name)})`)
      .all()
      .map((row) => {
        const column = row as {
          name: string;
          type: string;
          notnull: number;
          dflt_value: string | null;
          pk: number;
        };
        return {
          name: column.name,
          type: column.type,
          notNull: column.notnull === 1,
          defaultValue: column.dflt_value ?? null,
          primaryKey: column.pk,
        } satisfies ColumnSnapshot;
      })
      .sort((left, right) => left.name.localeCompare(right.name));
    const indexes = (
      db
        .prepare(
          "SELECT name, sql FROM sqlite_master WHERE type='index' AND tbl_name=? AND sql IS NOT NULL ORDER BY name",
        )
        .all(name) as Array<{ name: string; sql: string }>
    ).map((index) => ({
      name: index.name,
      sql: normalizeSchemaSql(index.sql),
    }));
    tables[name] = { columns, indexes };
  }
  return { tables };
}

export function buildTasksSchemaSnapshot(): DatabaseSnapshot {
  const db = new DatabaseSync(":memory:");
  try {
    runTasksDatabaseMigrations(db);
    return readDatabaseSchema(db);
  } finally {
    db.close();
  }
}

export function buildCliSessionStoreSchemaSnapshot(): DatabaseSnapshot {
  const db = new DatabaseSync(":memory:");
  try {
    // 与 migration-runner 的建表顺序一致：部分迁移 SQL 会引用账本表。
    db.exec(`create table if not exists schema_migration (
      id text primary key, checksum text not null, app_version text, time_applied integer not null
    )`);
    for (const migration of SQLITE_MIGRATIONS) {
      db.exec(migration.sql);
    }
    return readDatabaseSchema(db);
  } finally {
    db.close();
  }
}

/** 两个库都必须进快照：桌面任务库与 CLI 会话库由不同进程写入，缺一不可。 */
export function buildSchemaSnapshot(): SchemaSnapshot {
  return {
    schemaVersion: SQLITE_SCHEMA_SNAPSHOT_VERSION,
    databases: {
      tasks: buildTasksSchemaSnapshot(),
      "session-store": buildCliSessionStoreSchemaSnapshot(),
    },
  };
}
