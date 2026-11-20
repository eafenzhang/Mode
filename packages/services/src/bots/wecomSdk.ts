/**
 * `@wecom/aibot-node-sdk` 的**唯一懒加载点**（路线图 M4a 可选项：IM bot SDK 可选化）。
 *
 * 为什么单独收口一个模块：
 * - 启动路径不加载 SDK：`botsService` 静态引用 provider/runtime，若它们再静态引用 SDK，
 *   Host 一启动就会把 SDK 全量源码拉进模块图（feishu 侧同款问题已于此前改为
 *   `await import`，见 `feishuProvider.ts` 的启动注释）。收口后**未启用企微通道时
 *   SDK 完全不被加载**。
 * - 两处使用方（provider / channelRuntime）共享同一份缓存：动态 import 本就按模块缓存，
 *   这里再缓存 promise 是为了让「加载中」的并发调用收敛到同一次解析，也让守卫测试能
 *   断言「同一引用」。
 *
 * 在线升级：入口优先取 active 运行时（`imSdkRuntime.ts`），未安装/校验失败时回退随包
 * `node_modules`。缓存按解析结果分键——激活新版本后，下一次通道启动会重新加载。
 *
 * 类型面不受影响：`import type` 在编译期擦除，各文件仍可用
 * `import type { ... } from "@wecom/aibot-node-sdk"` 取类型（见 wecomConnection.ts）。
 */
import { pathToFileURL } from "node:url";

import { resolveImSdkEntry } from "./imSdkRuntime.js";

type WeComSdkModule = typeof import("@wecom/aibot-node-sdk");

interface CachedWeComSdk {
  /** undefined = 随包 node_modules；字符串 = 在线运行时的入口绝对路径。 */
  entry: string | undefined;
  promise: Promise<WeComSdkModule>;
}

let cachedWeComSdk: CachedWeComSdk | undefined;

/** 拉取企微 SDK 模块（首次调用触发真实加载，之后复用同一 promise）。 */
export function loadWeComSdk(): Promise<WeComSdkModule> {
  const entry = resolveImSdkEntry("@wecom/aibot-node-sdk");
  if (cachedWeComSdk && cachedWeComSdk.entry === entry) {
    return cachedWeComSdk.promise;
  }
  const promise = entry
    ? (import(pathToFileURL(entry).href) as Promise<WeComSdkModule>)
    : import("@wecom/aibot-node-sdk");
  const cached: CachedWeComSdk = { entry, promise };
  cachedWeComSdk = cached;
  // 加载失败不缓存失败态：下一次通道启动会重试，避免一次坏包永久拖垮通道。
  void promise.catch(() => {
    if (cachedWeComSdk === cached) cachedWeComSdk = undefined;
  });
  return promise;
}
