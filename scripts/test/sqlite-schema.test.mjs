import assert from "node:assert/strict";
import test from "node:test";

import {
  canonicalSchemaJson,
  compareSchemaSnapshots,
  formatSchemaViolation,
} from "../lib/sqlite-schema.mjs";

const column = (name, overrides = {}) => ({
  name,
  type: "TEXT",
  notNull: false,
  defaultValue: null,
  primaryKey: 0,
  ...overrides,
});
const index = (name, sql = `create index ${name} on t(a)`) => ({ name, sql });
const table = (columns, indexes = []) => ({ columns, indexes });
const snapshot = (databases) => ({ schemaVersion: 1, databases });

function baseSnapshot() {
  return snapshot({
    tasks: {
      tables: {
        t: table([column("id", { notNull: true, primaryKey: 1 }), column("a")], [index("idx_a")]),
      },
    },
    "session-store": {
      tables: {
        s: table([column("id", { primaryKey: 1 })]),
      },
    },
  });
}

test("相同快照没有违规也没有新增", () => {
  const result = compareSchemaSnapshots(baseSnapshot(), baseSnapshot());
  assert.deepEqual(result.violations, []);
  assert.deepEqual(result.added, []);
});

test("新增表、列、索引、数据库都允许", () => {
  const current = baseSnapshot();
  current.databases.tasks.tables.t.columns.push(column("b", { type: "INTEGER" }));
  current.databases.tasks.tables.t.indexes.push(index("idx_b", "create index idx_b on t(b)"));
  current.databases.tasks.tables.extra = table([column("x")]);
  current.databases.extra = { tables: { e: table([column("x")]) } };
  const result = compareSchemaSnapshots(baseSnapshot(), current);
  assert.deepEqual(result.violations, []);
  assert.ok(result.added.some((item) => item.kind === "column" && item.column === "b"));
  assert.ok(result.added.some((item) => item.kind === "index" && item.index === "idx_b"));
  assert.ok(result.added.some((item) => item.kind === "table" && item.table === "extra"));
  assert.ok(result.added.some((item) => item.kind === "database" && item.database === "extra"));
});

test("删表、删列、删库都违规", () => {
  const current = baseSnapshot();
  delete current.databases["session-store"];
  current.databases.tasks.tables.t.columns = [column("id", { notNull: true, primaryKey: 1 })];
  delete current.databases.tasks.tables.t;
  const result = compareSchemaSnapshots(baseSnapshot(), current);
  const kinds = result.violations.map((item) => item.kind);
  assert.ok(kinds.includes("database-removed"));
  assert.ok(kinds.includes("table-removed"));
});

test("删列与改列定义违规", () => {
  const current = baseSnapshot();
  current.databases.tasks.tables.t.columns = [
    column("id", { notNull: true, primaryKey: 1 }),
    column("a", { type: "INTEGER" }),
  ];
  const result = compareSchemaSnapshots(baseSnapshot(), current);
  assert.equal(result.violations.length, 1);
  assert.equal(result.violations[0].kind, "column-changed");
  assert.match(formatSchemaViolation(result.violations[0]), /列定义被修改/);
});

test("删索引与改索引 SQL 违规", () => {
  const removed = baseSnapshot();
  removed.databases.tasks.tables.t.indexes = [];
  const removedResult = compareSchemaSnapshots(baseSnapshot(), removed);
  assert.equal(removedResult.violations[0].kind, "index-removed");

  const changed = baseSnapshot();
  changed.databases.tasks.tables.t.indexes = [index("idx_a", "create index idx_a on t(b)")];
  const changedResult = compareSchemaSnapshots(baseSnapshot(), changed);
  assert.equal(changedResult.violations[0].kind, "index-changed");
});

test("canonical JSON 与对象键顺序无关", () => {
  const left = { schemaVersion: 1, databases: { tasks: { tables: {} } } };
  const right = { databases: { tasks: { tables: {} } }, schemaVersion: 1 };
  assert.equal(canonicalSchemaJson(left), canonicalSchemaJson(right));
});
