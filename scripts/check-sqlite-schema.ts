import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  canonicalSchemaJson,
  compareSchemaSnapshots,
  formatSchemaViolation,
} from "./lib/sqlite-schema.mjs";
import { buildSchemaSnapshot } from "./sqlite-schema-snapshot.ts";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const BASELINE_RELATIVE_PATH = "scripts/sqlite-schema-baseline.json";
const BASELINE_PATH = path.join(ROOT, BASELINE_RELATIVE_PATH);

interface Arguments {
  update: boolean;
  baseRef?: string;
}

function fail(message: string): never {
  console.error(`✗ ${message}`);
  process.exit(1);
}

function parseArguments(argv: string[]): Arguments {
  let update = false;
  let baseRef: string | undefined;
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--update") {
      update = true;
      continue;
    }
    if (argument === "--base-ref") {
      baseRef = argv[index + 1];
      index += 1;
      if (!baseRef) fail("--base-ref 需要 commit/tree-ish 参数");
      continue;
    }
    if (argument.startsWith("--base-ref=")) {
      baseRef = argument.slice("--base-ref=".length);
      if (!baseRef) fail("--base-ref 需要 commit/tree-ish 参数");
      continue;
    }
    fail(`未知参数：${argument}\n用法：pnpm check:sqlite-schema [--update] [--base-ref <ref>]`);
  }
  return { update, baseRef };
}

function readBaseline(): unknown | null {
  if (!existsSync(BASELINE_PATH)) return null;
  try {
    return JSON.parse(readFileSync(BASELINE_PATH, "utf8"));
  } catch (error) {
    fail(
      `基线文件无法解析：${BASELINE_RELATIVE_PATH}\n${error instanceof Error ? error.message : String(error)}`,
    );
  }
}

/**
 * 读取目标分支的基线。路径在目标树中不存在 = 首次引入；其它读取失败必须 fail closed，
 * 不能把无效 ref 或权限错误当成「没有基线」。
 */
function readBaselineAtRef(ref: string): unknown | null {
  try {
    const content = execFileSync("git", ["show", `${ref}:${BASELINE_RELATIVE_PATH}`], {
      cwd: ROOT,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    });
    return JSON.parse(content);
  } catch (error) {
    const stderr =
      typeof error === "object" && error !== null && "stderr" in error
        ? String((error as { stderr?: unknown }).stderr ?? "")
        : "";
    if (/does not exist in|exists on disk, but not in|Path .* does not exist/i.test(stderr)) {
      return null;
    }
    fail(
      `无法读取基线 ${ref}:${BASELINE_RELATIVE_PATH}\n${stderr || (error instanceof Error ? error.message : String(error))}`,
    );
  }
}

function assertAdditive(
  base: unknown,
  next: unknown,
  label: string,
): ReturnType<typeof compareSchemaSnapshots> {
  const result = compareSchemaSnapshots(base, next);
  if (result.violations.length > 0) {
    console.error(`✗ ${label} 存在破坏性 schema 变化：`);
    for (const violation of result.violations) {
      console.error(`  - ${formatSchemaViolation(violation)}`);
    }
    console.error("\n迁移只允许新增表/列/索引；删除或修改请走独立的数据迁移方案。");
    process.exit(1);
  }
  return result;
}

function main(): void {
  const { update, baseRef } = parseArguments(process.argv.slice(2));
  const current = buildSchemaSnapshot();
  const baseline = readBaseline();
  const baseBaseline = baseRef ? readBaselineAtRef(baseRef) : null;

  if (update) {
    if (baseline) assertAdditive(baseline, current, "现有基线");
    if (baseRef) {
      if (baseBaseline) {
        assertAdditive(baseBaseline, current, `目标基线 ${baseRef}`);
      } else {
        console.warn(`! ${baseRef} 中没有 ${BASELINE_RELATIVE_PATH}，按首次引入处理`);
      }
    }
    writeFileSync(BASELINE_PATH, `${JSON.stringify(current, null, 2)}\n`, "utf8");
    console.log(`✓ 已更新 ${BASELINE_RELATIVE_PATH}（${countTables(current)} 张表）`);
    return;
  }

  if (!baseline) {
    fail(`缺少 ${BASELINE_RELATIVE_PATH}；先运行 pnpm sqlite-schema:update 生成并提交基线`);
  }
  if (canonicalSchemaJson(current) !== canonicalSchemaJson(baseline)) {
    fail(
      `当前迁移与 ${BASELINE_RELATIVE_PATH} 不一致。\n` +
        "若只是新增表/列/索引：运行 pnpm sqlite-schema:update 并提交基线；\n" +
        "若是删除或修改：先修迁移，不能靠更新基线绕过。",
    );
  }
  if (baseRef && baseBaseline) {
    assertAdditive(baseBaseline, baseline, `目标基线 ${baseRef}`);
  }
  console.log(
    `✓ SQLite schema 只增检查通过：${Object.keys(current.databases).length} 个库、${countTables(current)} 张表`,
  );
}

function countTables(snapshot: ReturnType<typeof buildSchemaSnapshot>): number {
  return Object.values(snapshot.databases).reduce(
    (total, database) => total + Object.keys(database.tables).length,
    0,
  );
}

main();
