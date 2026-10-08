import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { requestMarketplaceAssetExists, requestMarketplaceJson } from "./marketplace.js";

/**
 * 目录条目补图标：Codex / Claude 市场的 marketplace.json 条目普遍不带 icon，
 * 但每个插件自己的 manifest 里通常声明了图标（`interface.composerIcon`，其次
 * `interface.logo` / `icon` / `logo`，路径相对插件目录）。这里按目录条目的
 * `source.path` 逐个读插件 manifest，把相对路径解析成市场仓库的 raw 直链写回
 * 条目的 `icon` 字段 —— UI 侧只信任 https，raw.githubusercontent 正好满足。
 *
 * 只有拿不到原始图标（插件确实没声明）时才退回字母图标，所以这里失败一律吞掉：
 * 图标是展示层信息，不能影响市场目录与安装链路。
 */

const PLUGIN_MANIFEST_CANDIDATES = [
  ".codex-plugin/plugin.json",
  ".claude-plugin/plugin.json",
  "plugin.json",
] as const;

const ICON_FIELD_CANDIDATES = [
  (manifest: Record<string, unknown>) => readInterfaceString(manifest, "composerIcon"),
  (manifest: Record<string, unknown>) => readInterfaceString(manifest, "logo"),
  (manifest: Record<string, unknown>) => readInterfaceString(manifest, "icon"),
  (manifest: Record<string, unknown>) => readString(manifest, "icon"),
  (manifest: Record<string, unknown>) => readString(manifest, "logo"),
] as const;

const ICON_EXTENSION_PATTERN = /\.(png|jpe?g|svg|webp|gif|avif)$/iu;

const ICON_FETCH_CONCURRENCY = 12;
const ICON_FETCH_DEADLINE_MS = 60_000;
// 目录再大也不至于无限补图：一次刷新最多补这么多（其余留到下次刷新）。
const ICON_FETCH_MAX_PER_REFRESH = 400;

interface PluginIconCacheFile {
  version: 1;
  // key: `${repo}#${ref}#${pluginPath}` -> 已解析的绝对图标 URL（或空串表示"确实没有图标"）
  icons: Record<string, string>;
}

function readString(source: Record<string, unknown>, key: string): string | undefined {
  const value = source[key];
  return typeof value === "string" && value.trim().length > 0 ? value.trim() : undefined;
}

function readInterfaceString(manifest: Record<string, unknown>, key: string): string | undefined {
  const block = manifest.interface;
  return block && typeof block === "object" && !Array.isArray(block)
    ? readString(block as Record<string, unknown>, key)
    : undefined;
}

/**
 * 图标路径的候选仓库相对路径（按序探测存在性，取第一个能真正取到的）。
 *
 * 生态里两种基准同时存在：多数插件把 `interface.composerIcon` 写成 `./assets/x.svg`
 * 表示**插件根目录**下的 assets（按 manifest 所在目录解析会 404），少数写成 `../assets/x`
 * 或把 assets 放在 manifest 同层。因此同时给出两个候选，由调用方探测可用性。
 */
export function resolveMarketplaceIconRepoPaths(
  pluginPath: string,
  manifestRelativePath: string,
  iconPath: string,
): string[] {
  if (!ICON_EXTENSION_PATTERN.test(iconPath) || /^(?:[a-z]+:)?\/\//iu.test(iconPath)) {
    return [];
  }
  const manifestDir = manifestRelativePath.includes("/")
    ? manifestRelativePath.slice(0, manifestRelativePath.lastIndexOf("/"))
    : "";
  const candidates = [
    normalizeRepoPath(`${pluginPath}/${manifestDir}/${iconPath}`),
    normalizeRepoPath(`${pluginPath}/${iconPath.replace(/^\.\//u, "")}`),
  ].filter((value): value is string => Boolean(value));
  return [...new Set(candidates)];
}

/** 逐段解析 `.` / `..`，越过仓库根目录则视为非法。 */
function normalizeRepoPath(path: string): string | null {
  const resolved: string[] = [];
  for (const segment of path.split("/")) {
    if (segment === "" || segment === ".") {
      continue;
    }
    if (segment === "..") {
      if (resolved.length === 0) {
        return null;
      }
      resolved.pop();
      continue;
    }
    resolved.push(segment);
  }
  return resolved.length > 0 ? resolved.join("/") : null;
}

/** 从插件 manifest 里取声明的图标路径（按约定顺序）。 */
export function readPluginManifestIconPath(manifest: Record<string, unknown>): string | null {
  for (const read of ICON_FIELD_CANDIDATES) {
    const value = read(manifest);
    if (value && !/^(?:[a-z]+:)?\/\//iu.test(value)) {
      return value;
    }
  }
  return null;
}

export function resolveRawGitHubUrl(repo: string, ref: string | undefined, path: string): string {
  const resolvedRef = ref?.trim() || "HEAD";
  return `https://raw.githubusercontent.com/${repo}/${resolvedRef}/${path}`;
}

function loadIconCache(storageRoot: string): PluginIconCacheFile {
  const path = join(storageRoot, "plugin-icons-cache.json");
  try {
    const parsed = JSON.parse(readFileSync(path, "utf8")) as PluginIconCacheFile;
    if (parsed && typeof parsed === "object" && parsed.version === 1 && parsed.icons) {
      return parsed;
    }
  } catch {
    // 缓存缺失/损坏都按空缓存处理，重新解析即可。
  }
  return { version: 1, icons: {} };
}

function saveIconCache(storageRoot: string, cache: PluginIconCacheFile): void {
  try {
    writeFileSync(
      join(storageRoot, "plugin-icons-cache.json"),
      `${JSON.stringify(cache, null, 2)}\n`,
      "utf8",
    );
  } catch {
    // 缓存写失败只影响下次刷新的开销，不影响图标本身。
  }
}

async function mapWithConcurrency<T>(
  items: readonly T[],
  limit: number,
  run: (item: T) => Promise<void>,
): Promise<void> {
  let cursor = 0;
  const workers = Array.from({ length: Math.min(limit, items.length) }, async () => {
    while (cursor < items.length) {
      const index = cursor;
      cursor += 1;
      const item = items[index];
      if (item !== undefined) {
        await run(item);
      }
    }
  });
  await Promise.all(workers);
}

/**
 * 给目录条目补上插件自带图标的绝对 URL。返回新的 manifest（不修改入参）。
 * 仅支持 github 来源（raw URL 可推导）；其他来源原样返回。
 *
 * 两级策略：
 *  1. 条目自己带 `icon`（社区目录普遍是 `./plugins/...` 形式的相对路径）= 插件原始图标，
 *     直接按仓库根解析成 raw 直链，零额外请求；
 *  2. 条目没带 icon（少数插件）才去读它的 plugin.json，从 `interface.composerIcon` 等
 *     字段推导图标路径（带存在性探测，并缓存结果）。
 * 两者都拿不到时保持无图标，UI 退回字母图标。
 */
export async function enrichMarketplaceManifestWithPluginIcons(input: {
  manifest: Record<string, unknown>;
  marketplace: string;
  ref?: string;
  repo: string;
  signal?: AbortSignal;
  storageRoot: string;
}): Promise<Record<string, unknown>> {
  const plugins = Array.isArray(input.manifest.plugins)
    ? input.manifest.plugins.filter(
        (entry): entry is Record<string, unknown> =>
          Boolean(entry) && typeof entry === "object" && !Array.isArray(entry),
      )
    : [];
  if (plugins.length === 0) {
    return input.manifest;
  }
  const cachePath = join(input.storageRoot, "plugin-icons-cache.json");
  const cache: PluginIconCacheFile = existsSync(cachePath)
    ? loadIconCache(input.storageRoot)
    : { version: 1, icons: {} };
  const cacheKeyOf = (repo: string, pluginPath: string) =>
    `${repo}#${input.ref ?? "HEAD"}#${pluginPath}`;
  const pending: Array<{ repo: string; pluginPath: string }> = [];
  const decorated = plugins.map((entry) => {
    const icon = readString(entry, "icon");
    if (icon) {
      // 目录条目自带的相对图标路径：解析成市场仓库的直链（绝对 URL / data URL 原样保留）。
      const repoRelative = resolveCatalogIconRepoPath(icon);
      return repoRelative
        ? { ...entry, icon: resolveRawGitHubUrl(input.repo, input.ref, repoRelative) }
        : entry;
    }
    // 目录内的插件（source 指向市场仓库里的相对路径）与 github 源插件（自带仓库地址）
    // 都按同一个规则去读它的 plugin.json；两者都没有时保持无图标。
    const pluginPath = resolvePluginSourcePath(entry);
    const entryRepo = pluginPath ? input.repo : resolvePluginSourceRepo(entry);
    if (!entryRepo) {
      return entry;
    }
    const resolvedPluginPath = pluginPath ?? "";
    const cached = cache.icons[cacheKeyOf(entryRepo, resolvedPluginPath)];
    if (cached !== undefined) {
      return cached ? { ...entry, icon: cached } : entry;
    }
    if (pending.length < ICON_FETCH_MAX_PER_REFRESH) {
      pending.push({ repo: entryRepo, pluginPath: resolvedPluginPath });
    }
    return entry;
  });
  if (pending.length === 0) {
    return { ...input.manifest, plugins: decorated };
  }

  const deadline = Date.now() + ICON_FETCH_DEADLINE_MS;
  const resolvedByPath = new Map<string, string>();
  await mapWithConcurrency(pending, ICON_FETCH_CONCURRENCY, async ({ repo, pluginPath }) => {
    if (Date.now() > deadline) {
      return;
    }
    const iconUrl = await resolvePluginIconUrl({
      pluginPath,
      ref: input.ref,
      repo,
      signal: input.signal,
    });
    resolvedByPath.set(cacheKeyOf(repo, pluginPath), iconUrl ?? "");
  });

  let changed = false;
  for (const [cacheKey, iconUrl] of resolvedByPath) {
    if (iconUrl) {
      changed = true;
    }
    cache.icons[cacheKey] = iconUrl;
  }
  if (changed || Object.keys(resolvedByPath).length > 0) {
    saveIconCache(input.storageRoot, cache);
  }

  return {
    ...input.manifest,
    plugins: decorated.map((entry) => {
      if (readString(entry, "icon")) {
        return entry;
      }
      const pluginPath = resolvePluginSourcePath(entry);
      const entryRepo = pluginPath ? input.repo : resolvePluginSourceRepo(entry);
      const iconUrl = entryRepo
        ? resolvedByPath.get(cacheKeyOf(entryRepo, pluginPath ?? ""))
        : undefined;
      return iconUrl ? { ...entry, icon: iconUrl } : entry;
    }),
  };
}

/**
 * 目录条目自带的 `icon` 解析：相对路径按仓库根解析（社区目录写 `./plugins/...`）；
 * 已经是绝对 URL / data URL 的返回 null（保持原样，由渲染侧的可信来源规则决定是否显示）。
 */
export function resolveCatalogIconRepoPath(icon: string): string | null {
  if (/^(?:[a-z][a-z0-9+.-]*:)?\/\//iu.test(icon) || icon.startsWith("data:")) {
    return null;
  }
  return normalizeRepoPath(icon.replace(/^\.\//u, ""));
}

/** 目录条目 github 源写法（`{source:"github", repo:"owner/repo"}`）的仓库地址。 */
function resolvePluginSourceRepo(entry: Record<string, unknown>): string | null {
  const source = entry.source;
  if (!source || typeof source !== "object" || Array.isArray(source)) {
    return null;
  }
  const kind = readString(source as Record<string, unknown>, "source");
  if (kind !== "github") {
    return null;
  }
  const repo = readString(source as Record<string, unknown>, "repo");
  return repo && /^[\w.-]+\/[\w.-]+$/u.test(repo) ? repo : null;
}

/** 目录条目的 `source.path`（Codex 写法 `{source:"local", path:"./plugins/x"}` 与裸字符串都支持）。 */
function resolvePluginSourcePath(entry: Record<string, unknown>): string | null {
  const source = entry.source;
  const raw =
    typeof source === "string" ? source : readString(source as Record<string, unknown>, "path");
  const trimmed = raw?.replace(/^\.\//u, "").replace(/\/+$/u, "");
  return trimmed && !/^(?:[a-z]+:)?\/\//iu.test(trimmed) ? trimmed : null;
}

async function resolvePluginIconUrl(input: {
  pluginPath: string;
  ref?: string;
  repo: string;
  signal?: AbortSignal;
}): Promise<string | null> {
  for (const manifestRelativePath of PLUGIN_MANIFEST_CANDIDATES) {
    const manifestUrl = resolveRawGitHubUrl(
      input.repo,
      input.ref,
      `${input.pluginPath}/${manifestRelativePath}`,
    );
    let manifest: unknown;
    try {
      manifest = await requestMarketplaceJson(manifestUrl, undefined, input.signal);
    } catch {
      continue;
    }
    if (!manifest || typeof manifest !== "object" || Array.isArray(manifest)) {
      continue;
    }
    const iconPath = readPluginManifestIconPath(manifest as Record<string, unknown>);
    if (!iconPath) {
      continue;
    }
    for (const repoIconPath of resolveMarketplaceIconRepoPaths(
      input.pluginPath,
      manifestRelativePath,
      iconPath,
    )) {
      const iconUrl = resolveRawGitHubUrl(input.repo, input.ref, repoIconPath);
      if (await requestMarketplaceAssetExists(iconUrl, input.signal)) {
        return iconUrl;
      }
    }
  }
  return null;
}
