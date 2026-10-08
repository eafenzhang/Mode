// 把 ZCode 官方插件目录（zcode-plugins-official）内置进随包资源：
//   1) 生成清单模块（去掉 per-install 的 cachePath 与 CDN icon，图标改为随包资源）；
//   2) 下载条目图标到 UI 资源目录并生成按插件名的映射；
//   3) 清单里的 filesystem 条目只保留本仓库真正内置的插件（其余会在安装时缺根目录）。
// 用法：node scripts/bundle-official-plugin-catalog.mjs
import { existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const catalogPath =
  process.env.ZCODE_OFFICIAL_CATALOG_PATH ??
  "C:/Users/Administrator/.zcode/cli/plugins/marketplaces/zcode-plugins-official/marketplace.json";
const iconsDir = join(root, "packages/ui/src/assets/plugin-icons/zcode-plugins-official");
const manifestModule = join(
  root,
  "apps/zcode-cli/packages/bootstrap/src/app/official-plugin-catalog.generated.ts",
);
const iconModule = join(root, "packages/ui/src/settings/zcodeOfficialPluginIcons.generated.ts");

/** 本仓库真正随包内置的插件（与 OFFICIAL_PLUGIN_DEFINITIONS 的 rootCandidates 对应）。 */
const bundledPluginDirs = [
  "browser-use-plugin",
  "node-repl-host",
  "superpowers-plugin",
].filter((dir) => existsSync(join(root, "apps/zcode-cli/packages", dir)));

const catalog = JSON.parse(readFileSync(catalogPath, "utf8"));
const entries = Array.isArray(catalog.plugins) ? catalog.plugins : Object.values(catalog.plugins);

const kept = [];
const droppedFilesystem = [];
for (const entry of entries) {
  const source = entry.source;
  const isFilesystem = source === "filesystem" || source === "sea";
  if (isFilesystem) {
    // filesystem 条目指向随包插件缓存：本仓库没有该插件目录就丢弃，避免出现点了就报错的条目。
    const dirKey = `${entry.name}`;
    const bundled = bundledPluginDirs.some((dir) =>
      dir.includes(dirKey.replace(/-plugin$/u, "")),
    );
    if (!bundled) {
      droppedFilesystem.push(entry.name);
      continue;
    }
  }
  const { cachePath: _cachePath, icon: _icon, ...rest } = entry;
  kept.push(rest);
}

// ── 1) 清单模块 ──
const manifestLines = [
  "// 本文件由 scripts/bundle-official-plugin-catalog.mjs 生成，请勿手改。",
  "// 内容为 ZCode 官方插件目录（zcode-plugins-official）的随包快照：",
  "// 去掉 per-install 的 cachePath 与远端 icon（图标按插件名匹配随包资源），",
  "// 条目 source 保持原样（url / filesystem），由既有安装链路解析。",
  "export const BUNDLED_OFFICIAL_PLUGIN_CATALOG = " +
    JSON.stringify({ name: catalog.name, plugins: kept }, null, 2) +
    " as const;",
  "",
];
writeFileSync(manifestModule, manifestLines.join("\n"), "utf8");

// ── 2) 图标 ──
mkdirSync(iconsDir, { recursive: true });
const PNG_MAGIC = Buffer.from([0x89, 0x50, 0x4e, 0x47]);
const iconSources = entries.filter(
  (entry) => typeof entry.icon === "string" && entry.icon.startsWith("https://"),
);
const downloaded = [];
const reused = [];
const failed = [];

async function fetchIcon(entry) {
  const target = join(iconsDir, `${entry.name}.png`);
  if (existsSync(target) && statSync(target).size > 0) {
    reused.push(entry.name);
    return;
  }
  for (let attempt = 1; attempt <= 3; attempt += 1) {
    try {
      const response = await fetch(entry.icon, { redirect: "follow" });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const body = Buffer.from(await response.arrayBuffer());
      if (body.length < 16 || !body.subarray(0, 4).equals(PNG_MAGIC)) {
        throw new Error(`not a PNG (${body.length}B)`);
      }
      writeFileSync(target, body);
      downloaded.push(entry.name);
      return;
    } catch (error) {
      if (attempt === 3) {
        failed.push(`${entry.name}: ${error instanceof Error ? error.message : String(error)}`);
      } else {
        await new Promise((resolve) => setTimeout(resolve, 400 * attempt));
      }
    }
  }
}

const queue = [...iconSources];
await Promise.all(
  Array.from({ length: 6 }, async () => {
    while (queue.length > 0) {
      const next = queue.shift();
      if (next) await fetchIcon(next);
    }
  }),
);

const available = entries
  .map((entry) => entry.name)
  .filter((name) => existsSync(join(iconsDir, `${name}.png`)))
  .sort((left, right) => left.localeCompare(right));

const iconLines = [
  "// 本文件由 scripts/bundle-official-plugin-catalog.mjs 生成，请勿手改：",
  "// ZCode 官方目录的插件图标随包内置，按插件名映射，运行时不再依赖 CDN。",
];
available.forEach((name, index) => {
  iconLines.push(`import icon${index} from "@/assets/plugin-icons/zcode-plugins-official/${name}.png";`);
});
iconLines.push("");
iconLines.push("/** 插件名 → 随包图标（仅用于 zcode-plugins-official 目录）。 */");
iconLines.push("export const ZCODE_OFFICIAL_PLUGIN_ICON_BY_NAME: Readonly<Record<string, string>> = {");
available.forEach((name, index) => {
  iconLines.push(`  ${JSON.stringify(name)}: icon${index},`);
});
iconLines.push("};");
iconLines.push("");
writeFileSync(iconModule, iconLines.join("\n"), "utf8");

console.log(`清单条目：${kept.length}（丢弃无内置目录的 filesystem 条目 ${droppedFilesystem.length}：${droppedFilesystem.join(", ") || "无"}）`);
console.log(`图标：下载 ${downloaded.length}、复用 ${reused.length}、失败 ${failed.length}，共 ${available.length} 个`);
if (failed.length > 0) for (const line of failed.slice(0, 10)) console.log("  ✗", line);
console.log(`生成：${manifestModule.replace(root, ".")} / ${iconModule.replace(root, ".")}`);
