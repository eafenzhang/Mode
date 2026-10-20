import { isAbsolute } from "node:path";

/**
 * 把壳传来的资源目录桥接成 Node 的 `process.resourcesPath`。
 *
 * 为什么需要这条桥：Windows 产品态 CUA runtime 的定位基准是 `process.resourcesPath`
 * （`resolveWindowsCuaRuntime` 的打包分支），这个字段由 Electron 注入；Tauri 的 Host 是
 * **纯 Node sidecar**，没有 Electron，字段恒为 `undefined` → 解析固定失败
 * `missing-resources-path` → 远程桌面 `rd_media_start` 永远拿不到 helper，主控只看到
 * 「对端不支持远程桌面」。壳本来就把同一个目录经 `MODE_HOST_RESOURCES_DIR` 传给 Host
 * （`crates/sidecar-host`），这里只做一次显式映射，不引入第二条路径来源。
 *
 * 已存在 `process.resourcesPath` 时不覆盖：那说明运行在确实会注入该字段的宿主里
 * （Electron / 未来壳自己注入），环境变量只是 sidecar 形态的补充。
 */
export const HOST_RESOURCES_DIR_ENV = "MODE_HOST_RESOURCES_DIR";

export interface ResourcesPathCarrier {
  resourcesPath?: string;
}

/**
 * 纯判定：已有值优先；否则取环境变量里的绝对路径。
 * 相对路径不采用：产品态解析要求绝对路径，静默接受相对值只会把错误推迟到运行期。
 */
export function resolveHostResourcesPath(
  env: NodeJS.ProcessEnv = process.env,
  current?: string,
): string | undefined {
  const existing = current?.trim();
  if (existing) {
    return existing;
  }
  const configured = env[HOST_RESOURCES_DIR_ENV]?.trim();
  if (!configured || !isAbsolute(configured)) {
    return undefined;
  }
  return configured;
}

/**
 * 写入宿主对象并返回最终生效的资源目录；没有可用来源时返回 `undefined`（保持原状）。
 * `target` 是测试注入点，产品调用只传 `env`，默认写真实 `process`。
 */
export function applyHostResourcesPath(
  env: NodeJS.ProcessEnv = process.env,
  target: ResourcesPathCarrier = process as unknown as ResourcesPathCarrier,
): string | undefined {
  const next = resolveHostResourcesPath(env, target.resourcesPath);
  if (next && next !== target.resourcesPath) {
    target.resourcesPath = next;
  }
  return target.resourcesPath;
}
