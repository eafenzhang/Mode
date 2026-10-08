import documentsIconUrl from "@/assets/plugin-icons/documents.png";
import imageSearchIconUrl from "@/assets/plugin-icons/image-search.png";
import pdfIconUrl from "@/assets/plugin-icons/pdf.png";
import pluginCreatorIconUrl from "@/assets/plugin-icons/plugin-creator.png";
import presentationsIconUrl from "@/assets/plugin-icons/presentations.png";
import spreadsheetsIconUrl from "@/assets/plugin-icons/spreadsheets.png";
import { canonicalPluginId } from "@mode/shared";
import { MODE_OFFICIAL_PLUGIN_ICON_BY_NAME } from "@/settings/modeOfficialPluginIcons.generated.js";
import { isTrustedImageUrl } from "@/lib/trustedImageUrl.js";

const OFFICIAL_PLUGIN_ICON_BY_ID: Readonly<Record<string, string>> = {
  "documents@mode-plugins-official": documentsIconUrl,
  "image-search@mode-plugins-official": imageSearchIconUrl,
  "pdf@mode-plugins-official": pdfIconUrl,
  "plugin-creator@mode-plugins-official": pluginCreatorIconUrl,
  "presentations@mode-plugins-official": presentationsIconUrl,
  "spreadsheets@mode-plugins-official": spreadsheetsIconUrl,
};

/**
 * 随包内置的插件图标（按插件名索引）：官方目录清单里的 icon 指向 CDN，
 * 客户端一律用随包资源覆盖；用户自行添加的 Claude 等目录若插件同名，
 * 也复用同一份原版图标。内置六件套按 id 维护，这里取出短名并入。
 */
const BUNDLED_PLUGIN_ICON_BY_NAME: Readonly<Record<string, string>> = {
  ...MODE_OFFICIAL_PLUGIN_ICON_BY_NAME,
  ...Object.fromEntries(
    Object.entries(OFFICIAL_PLUGIN_ICON_BY_ID).map(([id, url]) => [
      id.slice(0, id.lastIndexOf("@")),
      url,
    ]),
  ),
};

/** 官方市场的 id 形态：`<插件名>@mode-plugins-official`（改名前的旧 id 由查找时归一覆盖）。 */
const CATALOG_ICON_BY_ID: Readonly<Record<string, string>> = Object.fromEntries(
  Object.entries(BUNDLED_PLUGIN_ICON_BY_NAME).map(([name, url]) => [
    `${name}@mode-plugins-official`,
    url,
  ]),
);

/** 内置图标总表（按完整 id）：官方目录随包图标 + 客户端自带的六个官方插件图标。 */
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
    // 官方市场改名后，会话与存量设置里仍可能出现旧 id（name@zcode-plugins-official）：
    // 先按原样查，再按归一后的当前 id 查，两者共用同一份随包资源。
    const bundledIcon =
      ALL_BUNDLED_PLUGIN_ICONS[pluginId] ?? ALL_BUNDLED_PLUGIN_ICONS[canonicalPluginId(pluginId)];
    if (bundledIcon) return bundledIcon;
    // 同名兜底：用户自行添加的目录（如 Claude Code 插件）没有自带图标时，
    // 复用随包资源里的原版图标；同名不同源会共用同一个图标，这是有意的取舍。
    const at = pluginId.lastIndexOf("@");
    const byName = BUNDLED_PLUGIN_ICON_BY_NAME[at > 0 ? pluginId.slice(0, at) : pluginId];
    if (byName) return byName;
  }
  return isTrustedImageUrl(icon) ? icon : undefined;
}

/** Session 投影已完成身份匹配；仅放行固定打包资源，不放宽任意本地 URL。 */
export function isTrustedPluginIconSource(icon: string | undefined): icon is string {
  return Boolean(icon && TRUSTED_BUNDLED_PLUGIN_ICONS.has(icon)) || isTrustedImageUrl(icon);
}
