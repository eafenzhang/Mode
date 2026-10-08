// 把 Mode 官方插件目录（mode-plugins-official）随包内置：生成清单模块
//   apps/mode-cli/packages/bootstrap/src/app/official-plugin-catalog.generated.ts
//
// 关键约定：清单必须与官方逐条一致（"公开分段显示的插件与官方保持一致"）。这里
// 不挑条目、不裁剪：官方目录里有几个插件，随包清单就是几个，字段原样保留
// （含 description_i18n，中文文案由官方清单直接带来）。
//   只去掉两类字段：
//     - cachePath：per-install 的本地缓存路径，随包快照里没有意义；
//     - icon：官方指向 CDN，客户端不请求 CDN，图标改由随包资源按插件名匹配
//       （见 scripts/fetch-plugin-icons-by-name.mjs 与 packages/ui/src/lib/pluginIconSource.ts）。
// 截图/条目顺序都跟随官方清单，diff 时能一眼看出上游新增了哪个插件。
//
// 用法：node scripts/bundle-official-plugin-catalog.mjs
//   MODE_OFFICIAL_CATALOG_PATH 可指定清单来源（默认取本机市场缓存）。
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
// 市场 id 与目录名同名：改名后（mode-plugins-official）优先，旧目录（zcode-plugins-official）
// 作为回退——存量的本机缓存是被切换前的实例拉取的，仍是旧目录名。
const OFFICIAL_MARKETPLACE_ID = "mode-plugins-official";
const OFFICIAL_MARKETPLACE_ID_CANDIDATES = [OFFICIAL_MARKETPLACE_ID, "zcode-plugins-official"];
const marketplacesDir =
  process.env.MODE_MARKETPLACES_DIR ??
  [join(homedir(), ".mode", "cli", "plugins", "marketplaces"),
   join(homedir(), ".zcodium", "cli", "plugins", "marketplaces")].find((dir) => existsSync(dir)) ??
  join(homedir(), ".mode", "cli", "plugins", "marketplaces");
const catalogPath =
  process.env.MODE_OFFICIAL_CATALOG_PATH ??
  OFFICIAL_MARKETPLACE_ID_CANDIDATES.map((id) =>
    join(marketplacesDir, id, "marketplace.json"),
  ).find((path) => existsSync(path)) ??
  join(marketplacesDir, OFFICIAL_MARKETPLACE_ID, "marketplace.json");
const manifestModule = join(
  root,
  "apps/mode-cli/packages/bootstrap/src/app/official-plugin-catalog.generated.ts",
);

if (!existsSync(catalogPath)) {
  console.error(`官方目录清单不存在：${catalogPath}`);
  console.error("先在桌面端/CLI 里刷新一次官方市场，或设置 MODE_OFFICIAL_CATALOG_PATH。");
  process.exit(1);
}

const catalog = JSON.parse(readFileSync(catalogPath, "utf8"));
const entries = Array.isArray(catalog.plugins) ? catalog.plugins : Object.values(catalog.plugins);

const kept = entries.map((entry) => {
  const { cachePath: _cachePath, icon: _icon, ...rest } = entry;
  return rest;
});

const manifestLines = [
  "// 本文件由 scripts/bundle-official-plugin-catalog.mjs 生成，请勿手改。",
  `// 内容为 Mode 官方插件目录（${OFFICIAL_MARKETPLACE_ID}）的随包快照，共 ${kept.length} 条，`,
  "// 与官方清单逐条一致（不裁剪、不挑条目），只去掉 per-install 的 cachePath 与远端 icon：",
  "// 图标按插件名匹配随包资源，条目 source 保持原样，由既有安装链路解析。",
  // 快照声明的市场名统一写当前 id：来源清单可能是切换前拉取的旧目录（name 仍是旧 id）。
  "export const BUNDLED_OFFICIAL_PLUGIN_CATALOG = " +
    JSON.stringify({ name: OFFICIAL_MARKETPLACE_ID, plugins: kept }, null, 2) +
    " as const;",
  "",
];
writeFileSync(manifestModule, manifestLines.join("\n"), "utf8");

const filesystemEntries = kept.filter((entry) => entry.source === "filesystem" || entry.source === "sea");
const strippedIcons = entries.filter((entry) => typeof entry.icon === "string").length;
console.log(
  `随包官方清单：${kept.length} 条（filesystem ${filesystemEntries.length}、其余 ${kept.length - filesystemEntries.length}），` +
    `去掉 icon ${strippedIcons} 个 → ${manifestModule}`,
);
