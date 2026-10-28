// SQLite schema 快照比较的唯一实现。
//
// 迁移只允许加表、加列、加索引；删表/删列/改列定义/换索引都会让旧库升级失败。
// 这里不执行迁移，只比较 scripts/sqlite-schema-snapshot.ts 产出的结构化快照。

export const SQLITE_SCHEMA_SNAPSHOT_VERSION = 1;

export function normalizeSchemaSql(sql) {
  return String(sql ?? "")
    .replace(/\s+/g, " ")
    .trim();
}

function canonicalize(value) {
  if (Array.isArray(value)) return value.map(canonicalize);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, canonicalize(value[key])]),
    );
  }
  return value;
}

/** 与对象键顺序无关的稳定序列化，用于基线精确比较。 */
export function canonicalSchemaJson(value) {
  return JSON.stringify(canonicalize(value));
}

function columnKey(column) {
  return canonicalSchemaJson({
    name: column.name,
    type: column.type,
    notNull: column.notNull,
    defaultValue: column.defaultValue,
    primaryKey: column.primaryKey,
  });
}

function compareColumns({ database, table, baseColumns, currentColumns, violations, added }) {
  const currentByName = new Map((currentColumns ?? []).map((column) => [column.name, column]));
  const baseNames = new Set();
  for (const baseColumn of baseColumns ?? []) {
    baseNames.add(baseColumn.name);
    const currentColumn = currentByName.get(baseColumn.name);
    if (!currentColumn) {
      violations.push({
        kind: "column-removed",
        database,
        table,
        column: baseColumn.name,
      });
      continue;
    }
    if (columnKey(baseColumn) !== columnKey(currentColumn)) {
      violations.push({
        kind: "column-changed",
        database,
        table,
        column: baseColumn.name,
        before: baseColumn,
        after: currentColumn,
      });
    }
  }
  for (const currentColumn of currentColumns ?? []) {
    if (!baseNames.has(currentColumn.name)) {
      added.push({ kind: "column", database, table, column: currentColumn.name });
    }
  }
}

function compareIndexes({ database, table, baseIndexes, currentIndexes, violations, added }) {
  const currentByName = new Map((currentIndexes ?? []).map((index) => [index.name, index]));
  const baseNames = new Set();
  for (const baseIndex of baseIndexes ?? []) {
    baseNames.add(baseIndex.name);
    const currentIndex = currentByName.get(baseIndex.name);
    if (!currentIndex) {
      violations.push({ kind: "index-removed", database, table, index: baseIndex.name });
      continue;
    }
    if (normalizeSchemaSql(baseIndex.sql) !== normalizeSchemaSql(currentIndex.sql)) {
      violations.push({
        kind: "index-changed",
        database,
        table,
        index: baseIndex.name,
        before: normalizeSchemaSql(baseIndex.sql),
        after: normalizeSchemaSql(currentIndex.sql),
      });
    }
  }
  for (const currentIndex of currentIndexes ?? []) {
    if (!baseNames.has(currentIndex.name)) {
      added.push({ kind: "index", database, table, index: currentIndex.name });
    }
  }
}

/**
 * 比较基线快照与当前快照。
 * `violations` 是破坏性变化；`added` 是允许的新增项。
 */
export function compareSchemaSnapshots(base, current) {
  const violations = [];
  const added = [];
  const baseDatabases = base?.databases ?? {};
  const currentDatabases = current?.databases ?? {};

  for (const [database, baseDb] of Object.entries(baseDatabases)) {
    const currentDb = currentDatabases[database];
    if (!currentDb) {
      violations.push({ kind: "database-removed", database });
      continue;
    }
    const baseTables = baseDb?.tables ?? {};
    const currentTables = currentDb?.tables ?? {};
    for (const [table, baseTable] of Object.entries(baseTables)) {
      const currentTable = currentTables[table];
      if (!currentTable) {
        violations.push({ kind: "table-removed", database, table });
        continue;
      }
      compareColumns({
        database,
        table,
        baseColumns: baseTable?.columns,
        currentColumns: currentTable?.columns,
        violations,
        added,
      });
      compareIndexes({
        database,
        table,
        baseIndexes: baseTable?.indexes,
        currentIndexes: currentTable?.indexes,
        violations,
        added,
      });
    }
    for (const table of Object.keys(currentTables)) {
      if (!(table in baseTables)) added.push({ kind: "table", database, table });
    }
  }

  for (const database of Object.keys(currentDatabases)) {
    if (!(database in baseDatabases)) added.push({ kind: "database", database });
  }

  return { violations, added };
}

export function formatSchemaViolation(violation) {
  switch (violation.kind) {
    case "database-removed":
      return `数据库被移除：${violation.database}`;
    case "table-removed":
      return `表被移除：${violation.database}.${violation.table}`;
    case "column-removed":
      return `列被移除：${violation.database}.${violation.table}.${violation.column}`;
    case "column-changed":
      return `列定义被修改：${violation.database}.${violation.table}.${violation.column}`;
    case "index-removed":
      return `索引被移除：${violation.database}.${violation.table}.${violation.index}`;
    case "index-changed":
      return `索引定义被修改：${violation.database}.${violation.table}.${violation.index}`;
    default:
      return JSON.stringify(violation);
  }
}
