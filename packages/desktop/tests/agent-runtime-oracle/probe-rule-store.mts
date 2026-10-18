// M2 Task 6 / D1：黑盒观察 allow_project 写入的 `local_setting` 完整表结构。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-rule-store.mts
//
// 纪律：只运行旧实现并检查它**自己写出的** SQLite 文件（不读迁移实现）。
// 输出：`sqlite_master` 的 DDL、`PRAGMA table_info`、以及实际写入的行。
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";

import { startOracleRuntime } from "./driver.mjs";

const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-rule-"));
const cwd = await mkdtemp(join(tmpdir(), "mode-probe-rule-ws-"));
const target = join(cwd, "rule.txt");

const session = await startOracleRuntime({
  dataRoot,
  cwd,
  script: [
    {
      toolCalls: [
        { name: "Write", input: { file_path: target, content: "rule\n" } },
      ],
    },
    { text: "rule store done" },
  ],
  permissionReply: (params) => {
    const input = (params.input ?? {}) as Record<string, unknown>;
    return {
      decision: "allow",
      reason: "Approved for this project",
      permissionUpdates: [
        {
          type: "addRules",
          behavior: "allow",
          rules: [{ toolName: "Write", ruleContent: String(input.file_path ?? "") }],
        },
      ],
    };
  },
});

try {
  await session.send("probe rule store");
  const deadline = Date.now() + 30_000;
  while (
    !session.frames.some(
      (frame) => frame.type === "turn.completed" || frame.type === "turn.failed",
    )
  ) {
    if (Date.now() > deadline) throw new Error("probe：没有终态");
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
} finally {
  await session.stop();
}

const dbPath = join(dataRoot, ".mode", "cli", "db", "db.sqlite");
process.stdout.write(`\n########## rule store: ${dbPath} ##########\n`);
try {
  const db = new DatabaseSync(dbPath, { readOnly: true });
  const ddl = db
    .prepare("SELECT sql FROM sqlite_master WHERE name = 'local_setting'")
    .all();
  process.stdout.write(`\n-- sqlite_master --\n${JSON.stringify(ddl, null, 2)}\n`);
  const columns = db.prepare("PRAGMA table_info(local_setting)").all();
  process.stdout.write(`\n-- PRAGMA table_info --\n${JSON.stringify(columns, null, 2)}\n`);
  const rows = db.prepare("SELECT * FROM local_setting").all();
  process.stdout.write(`\n-- rows --\n${JSON.stringify(rows, null, 2)}\n`);
  db.close();
} finally {
  await rm(dataRoot, { recursive: true, force: true });
  await rm(cwd, { recursive: true, force: true });
}
