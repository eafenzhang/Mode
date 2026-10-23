/**
 * `@wecom/aibot-node-sdk` 的**唯一懒加载点**（路线图 M4a 可选项：IM bot SDK 可选化）。
 *
 * 为什么单独收口一个模块：
 * - 启动路径不加载 SDK：`botsService` 静态引用 provider/runtime，若它们再静态引用 SDK，
 *   Host 一启动就会把 SDK 全量源码拉进模块图（feishu 侧同款问题已于此前改为
 *   `await import`，见 `feishuProvider.ts` 的启动注释）。收口后**未启用企微通道时
 *   SDK 完全不被加载**，这是后续把 SDK 移出随包闭包的前置条件。
 * - 两处使用方（provider / channelRuntime）共享同一份缓存：动态 import 本就按模块缓存，
 *   这里再缓存 promise 是为了让「加载中」的并发调用收敛到同一次解析，也让守卫测试能
 *   断言「同一引用」。
 *
 * 类型面不受影响：`import type` 在编译期擦除，各文件仍可用
 * `import type { ... } from "@wecom/aibot-node-sdk"` 取类型（见 wecomConnection.ts）。
 */
type WeComSdkModule = typeof import("@wecom/aibot-node-sdk");

let wecomSdkPromise: Promise<WeComSdkModule> | undefined;

/** 拉取企微 SDK 模块（首次调用触发真实加载，之后复用同一 promise）。 */
export function loadWeComSdk(): Promise<WeComSdkModule> {
  wecomSdkPromise ??= import("@wecom/aibot-node-sdk");
  return wecomSdkPromise;
}
