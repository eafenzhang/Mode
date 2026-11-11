import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import test from "node:test";

import { SCHEMA_VERSION, migrateSchema } from "../src/schema.js";

test("schema 迁移可重复执行并标记 user_version", () => {
  const db = new DatabaseSync(":memory:");
  try {
    migrateSchema(db);
    migrateSchema(db);
    const row = db.prepare("PRAGMA user_version").get() as { user_version?: number };
    assert.equal(row.user_version, SCHEMA_VERSION);
  } finally {
    db.close();
  }
});

test("数据库版本高于当前代码时拒绝启动", () => {
  const db = new DatabaseSync(":memory:");
  try {
    db.exec("PRAGMA user_version = 99");
    assert.throws(() => migrateSchema(db), /高于当前支持/);
  } finally {
    db.close();
  }
});
