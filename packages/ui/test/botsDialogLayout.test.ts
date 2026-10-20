import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (relativePath: string): string =>
  readFileSync(new URL(relativePath, import.meta.url), "utf8");

test("SettingsRow 控制列允许换行，避免固定列宽时向右溢出", () => {
  const source = read("../src/settings/SettingsPageParts.tsx");
  assert.match(
    source,
    /flex w-full min-w-0 flex-wrap items-center justify-end gap-2/,
    "控制列必须有 min-w-0 + flex-wrap（BotsDialog 右栏横向溢出的根因）",
  );
});

test("BotsDialog 右栏禁止横向滚动，纵向滚动保持", () => {
  const source = read("../src/BotsDialog.tsx");
  assert.match(source, /overflow-x-hidden overflow-y-auto/);
});
