/**
 * `window.__TAURI__` 的最小类型声明。
 *
 * 骨架期用 `withGlobalTauri`（`tauri.conf.json`）暴露全局对象，而不是加
 * `@tauri-apps/api` 依赖：适配层本来就通过形参注入 `invoke` / `listen`
 * （见 `packages/ui/src/platform/tauri-invoke-shim.ts`），壳里只需要把这两个真实现喂进去。
 * 这样前端构建链不引入任何 Tauri 专属 npm 包。
 *
 * 只声明骨架期用到的两个成员，不做「完整镜像官方 API」的声明——没用到的方法写进来
 * 只会变成没人验证的假类型。
 */
import type { TauriInvoke, TauriListen } from "@mode/ui/tauri-platform";

export type TauriGlobal = {
  core: { invoke: TauriInvoke };
  event: { listen: TauriListen };
};

declare global {
  interface Window {
    __TAURI__?: TauriGlobal;
  }
}
