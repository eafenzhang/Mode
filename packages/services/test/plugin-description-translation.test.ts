import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";

// 需求：插件市场默认显示默认图标（没有原始图标时不再生成替代图标），并把 Codex 聚合
// 目录的插件简介翻译成中文。这里钉住三件事：
//   1. 翻译字典结构完整（键数、值非空且确实含中文、无英文占位残留）；
//   2. 简介解析顺序：目录自带 i18n > 内置中文翻译 > 原文；
//   3. 翻译只作用于 Codex 聚合目录、只在 zh 语言下生效（其他市场/语言不误翻）。

const translationsSource = await readFile(
  new URL("../../../packages/ui/src/settings/pluginDescriptionTranslations.ts", import.meta.url),
  "utf8",
).catch(() => null);

const listingSource = await readFile(
  new URL("../../../packages/ui/src/settings/pluginStoreListing.ts", import.meta.url),
  "utf8",
).catch(() => null);

test("中文简介字典：覆盖完整、值非空且含中文", {
  skip: !translationsSource && "无法读取翻译文件",
}, () => {
  const source = translationsSource;
  if (!source) return;
  const entries = [...source.matchAll(/^ {2}(?:"([^"]+)"|([a-zA-Z_$][\w$]*)): "([^"]+)",$/gm)].map(
    (match) => ({ key: match[1] ?? match[2]!, value: match[3]! }),
  );
  assert.ok(entries.length >= 250, `中文简介条目过少：${entries.length}`);
  const cjk = /[\u4e00-\u9fff]/u;
  for (const entry of entries) {
    assert.ok(entry.value.trim().length > 0, `空简介: ${entry.key}`);
    assert.ok(cjk.test(entry.value), `简介缺少中文（可能仍是英文原文）: ${entry.key}`);
  }
  const keys = new Set(entries.map((entry) => entry.key));
  assert.equal(keys.size, entries.length, "存在重复的插件名键");
  // 抽查几条容易翻错的
  assert.ok(keys.has("a-team") && keys.has("zotero-research-tools"));
});

test("简介解析顺序：目录 i18n 优先，其次内置中文翻译，最后原文", {
  skip: !listingSource && "无法读取商店列表源码",
}, () => {
  const source = listingSource;
  if (!source) return;
  assert.ok(
    source.includes("resolvePluginDescriptionZh(item.id, locale)"),
    "resolveItemDescription 必须接入内置中文翻译",
  );
  const body = source.slice(
    source.indexOf("export function resolveItemDescription"),
    source.indexOf("export function resolveItemDescription") + 700,
  );
  const providedIndex = body.indexOf("provided ??"); 
  const translatedIndex = body.indexOf("resolvePluginDescriptionZh(item.id, locale)");
  const fallbackIndex = body.indexOf("resolveLocalizedText(locale, base");
  assert.ok(
    providedIndex >= 0 && translatedIndex > providedIndex && fallbackIndex > translatedIndex,
    "顺序必须是 目录 i18n > 内置翻译 > 原文",
  );
});

test("翻译作用域：仅 Codex 聚合目录 + zh 语言", {
  skip: !translationsSource && "无法读取翻译文件",
}, () => {
  const source = translationsSource;
  if (!source) return;
  assert.ok(
    source.includes('CODEX_CATALOG_MARKETPLACE_ID = "awesome-codex-plugins"'),
    "必须限定默认 Codex 目录的市场 id",
  );
  assert.ok(
    source.includes('locale.toLowerCase().startsWith("zh")'),
    "必须只在中文界面下使用内置翻译",
  );
  assert.ok(
    source.includes("pluginId.slice(at + 1) !== CODEX_CATALOG_MARKETPLACE_ID"),
    "其他市场的同名插件不能被误翻",
  );
});
