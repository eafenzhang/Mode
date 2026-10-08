import documentsIconUrl from "@/assets/plugin-icons/documents.png";
import imageSearchIconUrl from "@/assets/plugin-icons/image-search.png";
import pdfIconUrl from "@/assets/plugin-icons/pdf.png";
import pluginCreatorIconUrl from "@/assets/plugin-icons/plugin-creator.png";
import presentationsIconUrl from "@/assets/plugin-icons/presentations.png";
import spreadsheetsIconUrl from "@/assets/plugin-icons/spreadsheets.png";
import { ZCODE_OFFICIAL_PLUGIN_ICON_BY_NAME } from "@/settings/zcodeOfficialPluginIcons.generated.js";
import { isTrustedImageUrl } from "@/lib/trustedImageUrl.js";

const OFFICIAL_PLUGIN_ICON_BY_ID: Readonly<Record<string, string>> = {
  "documents@zcode-plugins-official": documentsIconUrl,
  "image-search@zcode-plugins-official": imageSearchIconUrl,
  "pdf@zcode-plugins-official": pdfIconUrl,
  "plugin-creator@zcode-plugins-official": pluginCreatorIconUrl,
  "presentations@zcode-plugins-official": presentationsIconUrl,
  "spreadsheets@zcode-plugins-official": spreadsheetsIconUrl,
};

/**
 * 随包内置的插件图标（按插件名索引）：
 * 官方目录清单里的 icon 指向 CDN，客户端一律用随包资源覆盖；
 * 用户自行添加的 Claude 等目录若插件同名，也复用同一份原版图标。
 */
const CATALOG_ICON_BY_ID: Readonly<Record<string, string>> = Object.fromEntries(
  Object.entries(ZCODE_OFFICIAL_PLUGIN_ICON_BY_NAME).map(([name, url]) => [
    `${name}@zcode-plugins-official`,
    url,
  ]),
);

/** 内置图标总表：官方目录随包快照 + 客户端自带的六个官方插件图标。 */
const ALL_BUNDLED_PLUGIN_ICONS: Readonly<Record<string, string>> = {
  ...CATALOG_ICON_BY_ID,
  ...OFFICIAL_PLUGIN_ICON_BY_ID,
};

const TRUSTED_BUNDLED_PLUGIN_ICONS = new Set(Object.values(ALL_BUNDLED_PLUGIN_ICONS));

/** 按完整身份解析客户端自有图标，避免商店、候选和消息各自维护不同例外。 */
export function resolvePluginIconSource(
  pluginId: string | undefined,
  icon?: string,
): string | undefined {
  if (pluginId) {
    const bundledIcon = ALL_BUNDLED_PLUGIN_ICONS[pluginId];
    if (bundledIcon) return bundledIcon;
    // 同名兜底：用户自行添加的目录（如 Claude Code 插件）没有自带图标时，
    // 复用随包资源里的原版图标；同名不同源会共用同一个图标，这是有意的取舍。
    const at = pluginId.lastIndexOf("@");
    const byName = ALL_BUNDLED_PLUGIN_ICONS[at > 0 ? pluginId.slice(0, at) : pluginId];
    if (byName) return byName;
  }
  return isTrustedImageUrl(icon) ? icon : undefined;
}

/** Session 投影已完成身份匹配；仅放行固定打包资源，不放宽任意本地 URL。 */
export function isTrustedPluginIconSource(icon: string | undefined): icon is string {
  return Boolean(icon && TRUSTED_BUNDLED_PLUGIN_ICONS.has(icon)) || isTrustedImageUrl(icon);
}
