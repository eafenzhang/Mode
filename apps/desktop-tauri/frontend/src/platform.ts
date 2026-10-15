/**
 * 骨架前端的启动装配（规格 `docs/specs/tauri-platform-commands.md` §4）。
 *
 * 职责边界：本模块只做「拿到 Tauri 的 invoke/listen → 建平台服务 → 挂到 host 上」，
 * 不碰命令清单与 DOM（那两件事在 `harness.ts`）。这样 M3 接上 sidecar、能渲染真正的
 * 产品 UI 时，替换的是「挂什么」而不是「怎么拿到平台服务」。
 */
import { createTauriPlatformService, type TauriListen } from "@mode/ui/tauri-platform";

import type { TauriGlobal } from "./tauri-globals.js";

/** 取全局 Tauri API。缺失时明确报错——静默降级会让「shell 没配好」看起来像「命令返回空」。 */
function requireTauriGlobal(): TauriGlobal {
  const tauri = window.__TAURI__;
  if (!tauri?.core?.invoke || !tauri?.event?.listen) {
    throw new Error(
      "window.__TAURI__ 不可用：tauri.conf.json 需要 app.withGlobalTauri=true，" +
        "且渲染层必须由 Tauri webview（不是普通浏览器）加载。",
    );
  }
  return tauri;
}

export type FrontendPlatform = ReturnType<typeof createTauriPlatformService>;

/** 建平台服务（M1 的 10 条命令 + 一条深链事件通道）。 */
export function createPlatform(): FrontendPlatform {
  const tauri = requireTauriGlobal();
  // 形参注入是适配层的既有设计（利于注入测试替身），壳这边负责把真实现喂进去。
  return createTauriPlatformService(tauri.core.invoke, tauri.event.listen as TauriListen);
}
