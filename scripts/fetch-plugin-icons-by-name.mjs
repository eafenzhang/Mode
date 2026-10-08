// 按插件名下载并生成随包图标映射：图标取自本机已拉取的插件市场目录里各插件的条目图标，
// 客户端在渲染时按「插件名」兜底匹配（跨市场：官方源与用户添加的 Claude 源都能命中）。
// 用法：node scripts/fetch-plugin-icons-by-name.mjs [--force]
// 前置：本机曾拉取过这些市场（MODE_MARKETPLACES_DIR 可指向市场缓存目录，默认
// <home>/.mode/cli/plugins/marketplaces）。
import { existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const force = process.argv.includes("--force");
const root = join(dirname(fileURLToPath(import.meta.url)), "..");
// 先到先得顺序：官方目录的图标优先，其余目录（Claude 等）只补官方没有的名字。
const catalogs = ["zcode-plugins-official", "claude-plugins-official"].map((id) =>
  join(
    process.env.MODE_MARKETPLACES_DIR ?? join(homedir(), ".mode", "cli", "plugins", "marketplaces"),
    id,
    "marketplace.json",
  ),
);
const iconsDir = join(root, "packages/ui/src/assets/plugin-icons/mode-plugins-official");
const generated = join(root, "packages/ui/src/settings/modeOfficialPluginIcons.generated.ts");

const byName = new Map();
for (const catalogPath of catalogs) {
  if (!existsSync(catalogPath)) continue;
  try {
    const catalog = JSON.parse(readFileSync(catalogPath, "utf8"));
    for (const entry of catalog.plugins ?? []) {
      if (typeof entry?.name !== "string") continue;
      const icon =
        typeof entry.icon === "string" && entry.icon.startsWith("https://") ? entry.icon : null;
      // 先到先得：官方目录的图标优先，Claude 目录只补官方没有的名字。
      if (icon && !byName.has(entry.name)) {
        byName.set(entry.name, icon);
      }
    }
  } catch (error) {
    console.log(`跳过 ${catalogPath}：${error instanceof Error ? error.message : String(error)}`);
  }
}

const PNG_MAGIC = Buffer.from([0x89, 0x50, 0x4e, 0x47]);
mkdirSync(iconsDir, { recursive: true });
const downloaded = [];
const reused = [];
const failed = [];

async function fetchIcon(name, url) {
  const target = join(iconsDir, `${name}.png`);
  if (!force && existsSync(target) && statSync(target).size > 0) {
    reused.push(name);
    return;
  }
  for (let attempt = 1; attempt <= 3; attempt += 1) {
    try {
      const response = await fetch(url, { redirect: "follow" });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const body = Buffer.from(await response.arrayBuffer());
      if (body.length < 16 || !body.subarray(0, 4).equals(PNG_MAGIC)) {
        throw new Error(`not a PNG (${body.length}B)`);
      }
      writeFileSync(target, body);
      downloaded.push(name);
      return;
    } catch (error) {
      if (attempt === 3) {
        failed.push(`${name}: ${error instanceof Error ? error.message : String(error)}`);
      } else {
        await new Promise((resolve) => setTimeout(resolve, 400 * attempt));
      }
    }
  }
}

const queue = [...byName.entries()];
await Promise.all(
  Array.from({ length: 8 }, async () => {
    while (queue.length > 0) {
      const next = queue.shift();
      if (next) await fetchIcon(next[0], next[1]);
    }
  }),
);

// 生成映射：以磁盘上实际存在的文件为准（含上一轮已下载但已不在目录里的名字）。
const available = readdirSync(iconsDir)
  .filter((file) => file.endsWith(".png"))
  .map((file) => file.slice(0, -4))
  .sort((left, right) => left.localeCompare(right));

const lines = [
  "/* eslint-disable max-lines -- 随包图标映射由脚本生成：一个插件一行，拆分会打散「名字→资源」的对应关系。 */",
  "// 本文件由 scripts/fetch-plugin-icons-by-name.mjs 生成，请勿手改：",
  "// 插件图标随包内置，按「插件名」匹配（跨市场兜底）：官方源的清单 icon 指向 CDN，",
  "// 客户端不请求 CDN；用户自行添加的 Claude 等目录若插件同名，也直接复用这里的原版图标。",
];
available.forEach((name, index) => {
  lines.push(`import icon${index} from "@/assets/plugin-icons/mode-plugins-official/${name}.png";`);
});
lines.push("");
lines.push("/** 插件名 → 随包图标（同名跨市场复用）。 */");
lines.push("export const MODE_OFFICIAL_PLUGIN_ICON_BY_NAME: Readonly<Record<string, string>> = {");
available.forEach((name, index) => {
  lines.push(`  ${JSON.stringify(name)}: icon${index},`);
});
lines.push("};");
lines.push("");
writeFileSync(generated, lines.join("\n"), "utf8");

console.log(`图标：下载 ${downloaded.length}、复用 ${reused.length}、失败 ${failed.length}，随包共 ${available.length} 个`);
if (failed.length > 0) for (const line of failed.slice(0, 10)) console.log("  ✗", line);
