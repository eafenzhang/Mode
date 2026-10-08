import { isOfficialPlatformUrl } from "@mode/shared";
/** UI 远端图片只允许 HTTPS；失败时由各展示组件回退到本地图标。 */
export function isTrustedImageUrl(url: string | undefined): url is string {
  // 审计版不连接官方 CDN：历史市场图标也须在渲染前过滤。
  return !isOfficialPlatformUrl(url ?? "") && typeof url === "string" && url.startsWith("https://");
}
