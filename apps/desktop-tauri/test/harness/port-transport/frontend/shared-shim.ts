/**
 * 只给 harness 用的 `@mode/shared` 解析替身：把渲染层启动链用到的两个符号按**真实出处**重定向。
 *
 * 为什么不直接 alias 到 `packages/shared/src/index.ts`：那个 barrel 会拉进整个 shared 图
 * （含 node 侧模块），在浏览器里加载不起来。这里重定向的符号与 barrel 导出的是同一份定义，
 * 只是绕开了 barrel —— 不改变语义。
 */
export {
  databaseStartupPortPayloadSchema,
  databaseStartupStateSchema,
} from "../../../../../../packages/shared/src/database-startup.js";
export type { DatabaseStartupState } from "../../../../../../packages/shared/src/database-startup.js";
export { InternalChannels } from "../../../../../../packages/shared/src/channels.js";
