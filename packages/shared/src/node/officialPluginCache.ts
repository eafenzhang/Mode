import { existsSync } from "node:fs";
import { readdir } from "node:fs/promises";
import { join } from "node:path";
import {
  MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID,
  MODE_OFFICIAL_PLUGIN_MARKETPLACE_LEGACY_ID,
} from "../plugin-marketplaces.js";

interface OfficialPluginCacheRoot {
  /** 缓存目录名，即官方插件 name。 */
  name: string;
  /** 数字感知降序后的可用版本目录，首项为最新版本。 */
  versionRoots: string[];
}

/**
 * 官方插件缓存根：官方市场改名后，新目录（cache/mode-plugins-official）优先；
 * 新目录不存在时沿用旧目录（cache/zcode-plugins-official，存量插件文件在原地）。
 * 与 adapters 的 getPluginCacheDir 同一兜底口径，settings 侧直扫 cache 时共用。
 */
export function resolveOfficialPluginCacheRoot(pluginStorageRoot: string): string {
  const current = join(pluginStorageRoot, "cache", MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID);
  if (existsSync(current)) return current;
  const legacy = join(pluginStorageRoot, "cache", MODE_OFFICIAL_PLUGIN_MARKETPLACE_LEGACY_ID);
  return existsSync(legacy) ? legacy : current;
}

/**
 * 扫描 `<plugins storage>/cache/<官方市场 id>/<name>/<version>/`。
 * 内置官方插件由 CLI seed 到这里、没有 installed_plugins.json 记录，services 只读安装记录时会漏掉它们。
 * 版本目录跳过 CLI 的备份 / seed 锁 / 临时目录，并按数字感知降序排序，与 CLI 回退选取一致。
 */
export async function scanOfficialPluginCacheRoots(
  pluginStorageRoot: string,
): Promise<OfficialPluginCacheRoot[]> {
  const cacheRoot = resolveOfficialPluginCacheRoot(pluginStorageRoot);
  let pluginEntries;
  try {
    pluginEntries = await readdir(cacheRoot, { withFileTypes: true });
  } catch {
    return [];
  }

  const roots: OfficialPluginCacheRoot[] = [];
  for (const pluginEntry of pluginEntries) {
    if (!pluginEntry.isDirectory()) continue;
    const pluginDir = join(cacheRoot, pluginEntry.name);
    let versionEntries;
    try {
      versionEntries = await readdir(pluginDir, { withFileTypes: true });
    } catch {
      continue;
    }
    const versionRoots = versionEntries
      .filter((entry) => entry.isDirectory() && !isTransientCacheEntryName(entry.name))
      .map((entry) => entry.name)
      .sort((left, right) =>
        right.localeCompare(left, undefined, { numeric: true, sensitivity: "base" }),
      )
      .map((version) => join(pluginDir, version));
    if (versionRoots.length > 0) {
      roots.push({ name: pluginEntry.name, versionRoots });
    }
  }
  return roots.sort((left, right) => left.name.localeCompare(right.name));
}

function isTransientCacheEntryName(name: string): boolean {
  return name.includes(".backup") || name.includes(".seed-lock") || name.includes(".tmp-");
}
