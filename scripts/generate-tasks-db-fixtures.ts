/**
 * 生成 `tasks-db` 的 Rust 对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/session/tasksDatabase/migrations.ts`
 * - `packages/services/src/session/tasksDatabase/provider-selection-v2.ts`
 * - `scripts/sqlite-schema-snapshot.ts`
 *
 * ```bash
 * pnpm tasks-db:fixtures
 * cargo test -p tasks-db
 * ```
 *
 * 产出：
 * - `migrations`：迁移后内存库账本里的 id/checksum（checksum 由 TS 真实计算）。
 * - `snapshot`：`buildTasksSchemaSnapshot()` 的完整结构。
 * - `legacySelection.cases`：0002 导入后 `model_selection` 的原字符串（含 null）。
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { fileURLToPath } from "node:url";

import { runTasksDatabaseMigrations } from "../packages/services/src/session/tasksDatabase/migrations.js";
import { importLegacyAutomationSelections } from "../packages/services/src/session/tasksDatabase/provider-selection-v2.js";
import { buildTasksSchemaSnapshot } from "./sqlite-schema-snapshot.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT_DIR, "crates", "tasks-db", "fixtures", "tasks-db.json");

interface LegacyCaseInput {
  id: string;
  model: string | null;
  provider: string | null;
  thoughtLevel: string | null;
}

const legacyInputs: LegacyCaseInput[] = [
  { id: "custom-builtin-bigmodel", model: "custom:builtin:bigmodel:glm-4.6", provider: null, thoughtLevel: null },
  {
    id: "custom-builtin-start-plan",
    model: "custom:builtin:zai-start-plan:GLM-5.3",
    provider: null,
    thoughtLevel: "high",
  },
  {
    id: "custom-builtin-coding-plan",
    model: "custom:builtin:zai-coding-plan:glm-4.7",
    provider: null,
    thoughtLevel: null,
  },
  { id: "custom-builtin-unknown", model: "custom:builtin:unknown:glm-5", provider: null, thoughtLevel: null },
  { id: "custom-builtin-empty-provider", model: "custom:builtin::glm-5", provider: null, thoughtLevel: null },
  {
    id: "custom-builtin-encoded-model",
    model: "custom:builtin:bigmodel:glm%2F4.6",
    provider: null,
    thoughtLevel: null,
  },
  {
    id: "custom-builtin-extra-colons",
    model: "custom:builtin:zai:vendor/model:variant",
    provider: null,
    thoughtLevel: null,
  },
  { id: "custom-builtin-empty-model", model: "custom:builtin:bigmodel:", provider: null, thoughtLevel: null },
  {
    id: "custom-provider-model",
    model: "custom:openrouter:anthropic/claude-sonnet-4",
    provider: null,
    thoughtLevel: null,
  },
  { id: "custom-provider-encoded", model: "custom:my%20provider:model%2Fone", provider: null, thoughtLevel: null },
  {
    id: "custom-provider-decoded-utf8",
    model: "custom:p%2Fq:m%C3%A9",
    provider: null,
    thoughtLevel: null,
  },
  { id: "custom-provider-bad-percent", model: "custom:bad%ZZ:model", provider: null, thoughtLevel: null },
  { id: "custom-model-bad-percent", model: "custom:ok:bad%ZZ", provider: null, thoughtLevel: null },
  { id: "custom-no-separator", model: "custom:abc", provider: null, thoughtLevel: null },
  { id: "custom-empty-provider", model: "custom::model", provider: null, thoughtLevel: null },
  { id: "custom-chained-not-builtin", model: "custom:a:b:c", provider: null, thoughtLevel: null },
  { id: "custom-trailing-colon", model: "custom:prov:", provider: null, thoughtLevel: null },
  { id: "provider-model", model: "openai/gpt-5", provider: null, thoughtLevel: null },
  { id: "provider-model-level", model: "openai/gpt-5$high", provider: null, thoughtLevel: null },
  {
    id: "provider-model-level-padded",
    model: "openai/gpt-5$  high  ",
    provider: "ignored-column",
    thoughtLevel: "low",
  },
  { id: "provider-model-level-empty", model: "openai/gpt-5$   ", provider: null, thoughtLevel: null },
  { id: "provider-model-dollar-start", model: "openai/$high", provider: null, thoughtLevel: null },
  { id: "provider-model-dollar-end", model: "openai/gpt-5$", provider: null, thoughtLevel: null },
  { id: "provider-model-extra-slashes", model: "zai/models/glm-5", provider: null, thoughtLevel: null },
  { id: "provider-model-empty-provider", model: "/gpt-5", provider: null, thoughtLevel: null },
  {
    id: "provider-column-only",
    model: "  gpt-5  ",
    provider: "  openai  ",
    thoughtLevel: "  high  ",
  },
  { id: "provider-column-glm", model: "glm-4.6", provider: "glm", thoughtLevel: null },
  { id: "provider-column-mode", model: "gpt-5", provider: "mode", thoughtLevel: null },
  { id: "slash-overrides-glm", model: "zai/glm-5", provider: "glm", thoughtLevel: null },
  { id: "model-empty", model: "", provider: "openai", thoughtLevel: null },
  { id: "model-whitespace", model: "   ", provider: "openai", thoughtLevel: null },
  { id: "model-null", model: null, provider: null, thoughtLevel: null },
  { id: "model-null-with-provider", model: null, provider: "openai", thoughtLevel: "high" },
  { id: "custom-uppercase-prefix", model: "CUSTOM:openai:gpt-5", provider: "openai", thoughtLevel: null },
  { id: "provider-column-plain", model: "plain-model", provider: "openai", thoughtLevel: null },
  { id: "thought-level-whitespace", model: "openai/gpt-5", provider: null, thoughtLevel: "   " },
];

function collectMigrations(): Array<{ id: string; checksum: string }> {
  const db = new DatabaseSync(":memory:");
  try {
    runTasksDatabaseMigrations(db);
    return db
      .prepare("SELECT id, checksum FROM tasks_schema_migration ORDER BY id")
      .all() as unknown as Array<{ id: string; checksum: string }>;
  } finally {
    db.close();
  }
}

function collectLegacySelections(): Array<LegacyCaseInput & { expected: string | null }> {
  const db = new DatabaseSync(":memory:");
  try {
    runTasksDatabaseMigrations(db);
    const insert = db.prepare(
      `INSERT INTO automations
         (automation_id, cron_expr, prompt, workspace_key, workspace_path,
          model, provider, thought_level, created_at, updated_at)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
    );
    for (const input of legacyInputs) {
      insert.run(
        input.id,
        "0 0 * * *",
        "prompt",
        "workspace-key",
        "/workspace",
        input.model,
        input.provider,
        input.thoughtLevel,
        1,
        1,
      );
    }
    importLegacyAutomationSelections(db);
    const select = db.prepare("SELECT model_selection FROM automations WHERE automation_id=?");
    return legacyInputs.map((input) => {
      const row = select.get(input.id) as unknown as { model_selection: string | null } | undefined;
      if (!row) throw new Error(`missing automation row: ${input.id}`);
      return { ...input, expected: row.model_selection };
    });
  } finally {
    db.close();
  }
}

const fixture = {
  generatedBy: "scripts/generate-tasks-db-fixtures.ts",
  migrations: collectMigrations(),
  snapshot: buildTasksSchemaSnapshot(),
  legacySelection: { cases: collectLegacySelections() },
};

mkdirSync(path.dirname(OUTPUT), { recursive: true });
writeFileSync(OUTPUT, `${JSON.stringify(fixture, null, 2)}\n`);
console.log(
  `[tasks-db] migrations=${fixture.migrations.length} legacyCases=${fixture.legacySelection.cases.length} → ${path.relative(ROOT_DIR, OUTPUT)}`,
);
