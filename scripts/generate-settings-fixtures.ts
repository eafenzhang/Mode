/**
 * 生成 settings 的 Rust 侧黄金数据：默认值、顶层允许键、以及「合并 + 打补丁」的期望结果。
 *
 * 真源是 TS 的 zod schema（`appSettingsSchema` / `appSettingsPatchSchema`）。Rust 侧只负责
 * 与这些结果一致，不允许只改 Rust 让用例变绿。
 *
 * ```bash
 * pnpm settings:fixtures
 * cargo test -p settings-store
 * ```
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  appSettingsPatchSchema,
  appSettingsSchema,
} from "../packages/shared/src/validationAppSettings.js";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT, "crates", "settings-store", "fixtures", "settings.json");
const MAX_RECENT_PROJECTS = 10;

/**
 * `appSettingsSchema` 是 `z.preprocess(..., objectSchema)`：
 * zod v4 的内层对象在 `_def.out.shape`，v3 在 `_def.schema.shape`。
 */
const schemaDef = appSettingsSchema as unknown as {
  _def: { schema?: { shape: Record<string, unknown> }; out?: { shape: Record<string, unknown> } };
};
const innerShape = schemaDef._def.schema?.shape ?? schemaDef._def.out?.shape;
if (!innerShape) {
  throw new Error("无法从 appSettingsSchema 取到顶层字段表（zod 结构变化？）");
}
const allowedKeys = Object.keys(innerShape);

const defaults = appSettingsSchema.parse({});

interface Case {
  name: string;
  persisted: unknown;
  patch: unknown;
}

const cases: Case[] = [
  { name: "defaults-only", persisted: null, patch: {} },
  { name: "locale", persisted: null, patch: { locale: "en-US" } },
  {
    name: "recent-projects-dedupe-and-cap",
    persisted: {
      recentProjects: [
        "/a",
        "/b",
        "/a",
        "/c",
        "/d",
        "/e",
        "/f",
        "/g",
        "/h",
        "/i",
        "/j",
        "/k",
        "/l",
      ],
    },
    patch: {},
  },
  {
    name: "terminal-and-archive",
    persisted: null,
    patch: {
      terminalFontFamily: "Cascadia Code",
      taskAutoArchiveEnabled: true,
      taskAutoArchiveOlderThanDays: 30,
    },
  },
  { name: "unknown-patch-key-is-stripped", persisted: null, patch: { notARealSetting: true } },
  { name: "optional-field-survives", persisted: { dataBaseDir: "D:/ModeData" }, patch: {} },
];

const entries = cases.map((testCase) => {
  const current = appSettingsSchema.parse(testCase.persisted ?? {});
  const patch = appSettingsPatchSchema.parse(testCase.patch);
  const merged = appSettingsSchema.parse({ ...current, ...patch });
  if (Array.isArray(merged.recentProjects)) {
    // 与 settingService.update 的收尾一致：去重后只留最近 10 个。
    merged.recentProjects = [...new Set(merged.recentProjects)].slice(0, MAX_RECENT_PROJECTS);
  }
  return { name: testCase.name, persisted: testCase.persisted, patch: testCase.patch, expected: merged };
});

mkdirSync(path.dirname(OUTPUT), { recursive: true });
writeFileSync(
  OUTPUT,
  `${JSON.stringify({ generatedBy: "scripts/generate-settings-fixtures.ts", allowedKeys, defaults, cases: entries }, null, 2)}\n`,
);
console.log(
  `[settings] keys=${allowedKeys.length} cases=${entries.length} → ${path.relative(ROOT, OUTPUT)}`,
);
