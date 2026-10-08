/**
 * 插件详情页「组件清单」区的呈现态判定。
 *
 * 候选（未安装）条目靠 `plugins/describe` 现场解析组件；但有两类条目在本分支**确定**
 * 拿不到安装包，describe 必然失败：
 * - 未随包（bundledUnavailable）：磁盘上没有随包安装包；
 * - 源已下线（sourceUnavailable）：source 指向已下线官方平台，describe 要下载 zip，
 *   HTTP 出口断言必然拒绝。
 * 把这种「本来就没有」渲染成可重试的加载失败，会让用户以为重试能成功（实测点击重试
 * 每次都失败），因此各自单独成态，走固定说明、跳过 describe。
 */
export type PluginDetailComponentsState =
  | "unbundled"
  | "sourceUnavailable"
  | "loading"
  | "error"
  | "ready";

/** 详情页组件区呈现态：确定拿不到包的两类优先，其余按 describe 进度。 */
export function resolvePluginDetailComponentsState(input: {
  bundledUnavailable?: boolean | undefined;
  sourceUnavailable?: boolean | undefined;
  /** 已安装插件带权威组件枚举（ModePluginInfo.components），不需要 describe。 */
  hasRuntimeInfo: boolean;
  describeStatus?: "loading" | "loaded" | "error" | undefined;
}): PluginDetailComponentsState {
  if (input.hasRuntimeInfo) return "ready";
  // 未随包与源已下线互斥（source 决定来源类型），按「本地没有包」优先解释以防并存。
  if (input.bundledUnavailable === true) return "unbundled";
  if (input.sourceUnavailable === true) return "sourceUnavailable";
  if (input.describeStatus === "error") return "error";
  // 未请求过（undefined）与请求中都按加载中渲染：详情页打开即发起 describe。
  return input.describeStatus === "loaded" ? "ready" : "loading";
}
