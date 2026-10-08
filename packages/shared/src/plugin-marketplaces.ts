import { isOfficialServiceEnabled } from "./officialPlatformPolicy.js";

export interface DefaultPluginMarketplace {
  id: string;
  source: string;
  name: string;
  description: string;
  pluginCount: number;
  lastUpdated?: string;
}

export const MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID = "zcode-plugins-official";

/** Settings 三类资源发现共用；Bootstrap 单测与官方 definition 的 defaultEnabled 机械对照。 */
export const DEFAULT_ENABLED_OFFICIAL_PLUGIN_IDS: ReadonlySet<string> = new Set([
  "browser-use@zcode-plugins-official",
  "image-search@zcode-plugins-official",
  "documents@zcode-plugins-official",
  "pdf@zcode-plugins-official",
  "presentations@zcode-plugins-official",
  "spreadsheets@zcode-plugins-official",
  // node_repl 宿主：不进市场、不对用户露出，也不贡献任何 skill/command/subagent，但必须
  // 始终可用 —— node_repl 的注册门禁是「Browser Use 或 Computer Use 任一启用」，宿主自己
  // 不参与那个判断。Browser Use 默认开着，宿主若默认关就等于它上来就没有宿主。
  "node-repl-host@zcode-plugins-official",
  "skill-creator@zcode-plugins-official",
  "plugin-creator@zcode-plugins-official",
  "zcode-guide@zcode-plugins-official",
  // 电脑控制回退为默认关闭，故 computer-use 不在此名单内。
  // 该集合必须与 official-plugin-definitions.ts 里标了 defaultEnabled 的插件逐一对应，
  // bootstrap 的「Settings 默认启用集合与 CLI 的官方插件声明一致」单测机械对照两者。
]);

/**
 * Codex 插件市场（`.agents/plugins/marketplace.json` 约定，codex plugin marketplace add 生成）。
 * id 必须与市场 manifest 里声明的 name 一致：addMarketplace 以 manifest.name 作为市场 id，
 * 预声明的 id 对不上会在刷新时多出一个挂空记录。
 *
 * 默认指向社区聚合目录（awesome-codex-plugins，272 个插件）：用户要的是"Codex 生态的全部插件"，
 * 单个插件作者的仓库只能看到一两个。聚合仓体量大（>100MB），目录读取走 raw manifest 快路径
 * （见 CLI 侧 requestGitHubRawMarketplaceManifest），插件源码在安装时按需稀疏检出。
 */
export const CODEX_PLUGIN_MARKETPLACE_ID = "awesome-codex-plugins";
export const CODEX_PLUGIN_MARKETPLACE_SOURCE = "hashgraph-online/awesome-codex-plugins";


/** Claude Code 官方插件目录：不再是默认源；用户自己添加后必须长期保留（见退役清单说明）。 */
export const CLAUDE_PLUGIN_MARKETPLACE_ID = "claude-plugins-official";
export const CLAUDE_PLUGIN_MARKETPLACE_SOURCE = "anthropics/claude-plugins-official";

// 官方市场来源定义保留在表里只为让 id/顺序等结构兼容；官方平台服务已整体下线，
// 它永远不进入默认市场集合（见 resolveDefaultPluginMarketplaces），也不会被 seed。
export const DEFAULT_PLUGIN_MARKETPLACES: DefaultPluginMarketplace[] = [
  {
    // ZCode 官方唯一市场：本地 seed 分片与 CDN 分片在 Agent storage 内合并。
    // CDN manifest 的 name 必须与该 canonical id 一致。
    id: MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID,
    source: "https://cdn-zcode.z.ai/zcode/official-plugin/marketplace.json",
    name: MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID,
    description: "Official ZCode plugins marketplace: built-in and community plugins for ZCode.",
    pluginCount: 0,
  },


];

/**
 * 已退役的默认市场：曾作为默认源预置、但不再预置的来源。种子阶段按"id + 完全相同的 source"
 * 精确清理。清理分不清「当年自动种下的」和「用户自己按同一来源登记的」，所以只列那些
 * 用户不该再看到的来源：Claude 官方目录不在此列 —— 它正是个人分段要给用户用的东西，
 * 登记一次就得长期保留，否则每次 overview 都会把用户刚加回来的源删掉。
 */
export const RETIRED_DEFAULT_MARKETPLACES: ReadonlyArray<{ id: string; source: string }> = [
  // 早期默认源：只有一个插件的示例仓库，已换成聚合目录。
  { id: "xiu86-codex-plugins", source: "xiu86/codex-plugins" },
  // Codex 聚合目录（272 个插件）不再预置：既不是官方目录，也不属于个人分段的既定内容。
  { id: CODEX_PLUGIN_MARKETPLACE_ID, source: CODEX_PLUGIN_MARKETPLACE_SOURCE },
];

/**
 * 默认插件市场集合：官方来源恒被剔除（官方平台服务已下线，没有开关可以恢复），
 * 只保留 Codex 聚合目录等非官方来源；本地内置插件与个人来源不受影响。
 */
export function resolveDefaultPluginMarketplaces(): DefaultPluginMarketplace[] {
  return DEFAULT_PLUGIN_MARKETPLACES.filter(
    (marketplace) =>
      marketplace.id !== MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID ||
      isOfficialServiceEnabled("marketplace"),
  );
}

// 商店「公开」分段只有一个 ZCode 官方市场 id，内置与 CDN 不再拆分身份。
export const PUBLIC_STORE_MARKETPLACE_IDS = [MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID] as const;

export function isPublicStoreMarketplaceId(id: string): boolean {
  return (PUBLIC_STORE_MARKETPLACE_IDS as readonly string[]).includes(id);
}

/**
 * 随应用预置的市场（官方 + Codex 格式源）。它们由 ensureDefaultPluginMarketplaces
 * 在每次 overview 时补种：允许删除只会造成「删了又在刷新后回来」的困惑，UI 因此不给移除入口。
 */
export const BUILTIN_DEFAULT_MARKETPLACE_IDS = [MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID] as const;

export function isBuiltinDefaultMarketplaceId(id: string): boolean {
  return (BUILTIN_DEFAULT_MARKETPLACE_IDS as readonly string[]).includes(id);
}

export function isCodexPluginMarketplaceId(id: string): boolean {
  return id === CODEX_PLUGIN_MARKETPLACE_ID;
}

/**
 * 需要编程套餐（付费套餐）才好用的插件：官方目录条目用 `requiresPaidPlan: true` 标注
 * （金融与企业那批，含同花顺/天眼查/Wind 等）。
 *
 * Mode 里官方平台已整体下线，套餐无法开通，这类插件装上也只能报错，因此商店不把它们
 * 作为候选展示。判定只看条目自带的标记，不看来源市场：用户自己添加的目录若带同样标记，
 * 一视同仁。清单本身保留这个字段（随包官方清单逐条照抄官方），过滤发生在展示/候选层。
 */
export function listingRequiresPaidPlan(
  listing: { requiresPaidPlan?: boolean } | undefined | null,
): boolean {
  return listing?.requiresPaidPlan === true;
}

