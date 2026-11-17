// C2 Task 0：fresh-install 迁移账本观察（空隔离数据根启动 oracle，读取 db.sqlite）。
//
// 用法：pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c2-fresh-install.mts
//
// 只读 `sqlite_master` 与 `schema_migration`；**不读** TypeScript 迁移实现。
// 第二次启动复用同一数据根，验证账本不重写（id/checksum/行数不变）。
import { DatabaseSync } from "node:sqlite";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { startC2Harness, writeC2Fixture } from "./probe-c2-support.mjs";
import type { JsonObject } from "./session-lifecycle-fixtures.mjs";

function readLedger(dbPath: string): JsonObject {
  const db = new DatabaseSync(dbPath, { readOnly: true });
  try {
    const rows = db
      .prepare(
        "select id, checksum, app_version, time_applied from schema_migration order by rowid",
      )
      .all() as JsonObject[];
    return {
      rowCount: rows.length,
      rows: rows.map((row) => ({
        id: row.id,
        checksum: row.checksum,
        app_version: row.app_version,
        time_applied: "<time>",
      })),
      ids: rows.map((row) => row.id),
      checksums: rows.map((row) => row.checksum),
      appVersions: [...new Set(rows.map((row) => row.app_version))],
      timeAppliedMonotonic: rows.every(
        (row, index) =>
          index === 0 || Number(row.time_applied) >= Number(rows[index - 1]?.time_applied),
      ),
      dbPathNormalized: `<data-root>/.mode/cli/db/db.sqlite`,
    };
  } finally {
    db.close();
  }
}

function readSqliteMaster(dbPath: string): JsonObject {
  const db = new DatabaseSync(dbPath, { readOnly: true });
  try {
    const entries = db
      .prepare(
        "select type, name, tbl_name, sql from sqlite_master " +
          "where name not like 'sqlite_%' order by type, name",
      )
      .all() as JsonObject[];
    const pragmas = {
      user_version: (db.prepare("pragma user_version").get() as JsonObject).user_version ?? null,
      journal_mode: (db.prepare("pragma journal_mode").get() as JsonObject).journal_mode ?? null,
      page_size: (db.prepare("pragma page_size").get() as JsonObject).page_size ?? null,
      foreign_keys: (db.prepare("pragma foreign_keys").get() as JsonObject).foreign_keys ?? null,
    };
    return { entryCount: entries.length, entries, pragmas };
  } finally {
    db.close();
  }
}

function startupPhases(captureState: JsonObject): JsonObject {
  const storage = (captureState.startupStorage as JsonObject[] | undefined) ?? [];
  return {
    count: storage.length,
    phases: storage.map((frame) => frame.phase),
    sequence: storage.map((frame) => frame.sequence),
  };
}

const dataRoot = await mkdtemp(join(tmpdir(), "mode-c2-fresh-"));
const cwd = await mkdtemp(join(tmpdir(), "mode-c2-fresh-ws-"));
try {
  const first = await startC2Harness({
    prefix: "fresh-first",
    dataRoot,
    cwd,
    script: [{ text: "fresh install answer" }],
  });
  let firstLedger: JsonObject;
  let firstMaster: JsonObject;
  let firstStartup: JsonObject;
  let firstIsolation: JsonObject;
  try {
    firstLedger = readLedger(first.dbPath);
    firstMaster = readSqliteMaster(first.dbPath);
    firstStartup = startupPhases(first.captureState as unknown as JsonObject);
    firstIsolation = first.isolation();
  } finally {
    await first.stop();
  }

  const second = await startC2Harness({
    prefix: "fresh-second",
    dataRoot,
    cwd,
    script: [{ text: "second start answer" }],
  });
  let secondLedger: JsonObject;
  let secondMaster: JsonObject;
  let secondStartup: JsonObject;
  let secondIsolation: JsonObject;
  try {
    secondLedger = readLedger(second.dbPath);
    secondMaster = readSqliteMaster(second.dbPath);
    secondStartup = startupPhases(second.captureState as unknown as JsonObject);
    secondIsolation = second.isolation();
  } finally {
    await second.stop();
  }

  const fixture = {
    case: "fresh-install-ledger",
    note:
      "C2 Task 0 oracle 捕获：空隔离数据根首次启动创建的 session-store sqlite_master 与 " +
      "schema_migration 账本（id/checksum/app_version/time_applied），以及同一数据根第二次启动" +
      "不重写账本。time_applied 是机器时间，夹具折叠为 <time> 并记录相对首行的偏移；" +
      "ledger 的判定字段是 id/checksum/app_version/顺序。",
    input: { dataRoot: "mkdtemp(empty)", mode: "fresh-install → restart" },
    expect: {
      firstStart: {
        isolation: firstIsolation,
        startup: firstStartup,
        ledger: firstLedger,
        sqliteMaster: firstMaster,
      },
      secondStart: {
        isolation: secondIsolation,
        startup: secondStartup,
        ledger: secondLedger,
        sqliteMaster: secondMaster,
      },
      ledgerStable:
        JSON.stringify(firstLedger.rows) === JSON.stringify(secondLedger.rows) &&
        firstLedger.rowCount === secondLedger.rowCount,
      sqliteMasterStable:
        JSON.stringify(firstMaster.entries) === JSON.stringify(secondMaster.entries),
    },
  };
  const target = await writeC2Fixture("fresh-install-ledger", fixture);
  process.stdout.write(
    `${JSON.stringify(
      {
        fixture: target,
        first: {
          ledgerRowCount: firstLedger.rowCount,
          ids: firstLedger.ids,
          checksums: firstLedger.checksums,
          appVersions: firstLedger.appVersions,
          sqliteMasterEntryCount: firstMaster.entryCount,
          pragmas: firstMaster.pragmas,
          startup: {
            count: firstStartup.count,
            phases: firstStartup.phases,
          },
        },
        second: {
          ledgerRowCount: secondLedger.rowCount,
          startup: { count: secondStartup.count, phases: secondStartup.phases },
        },
        ledgerStable: fixture.expect.ledgerStable,
        sqliteMasterStable: fixture.expect.sqliteMasterStable,
      },
      null,
      2,
    )}\n`,
  );
} finally {
  if (!process.argv.includes("--keep")) {
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
}
