import { materializeModeBuiltinProviderConfig } from "@mode/services/node";

declare const __MODE_BUILTIN_PROVIDER_CONFIG_JSON__: string | undefined;

interface MaterializeBundledModeBuiltinProviderConfigOptions {
  readonly environmentConfigRoot: string;
  readonly content: string;
}

/** 返回构建时嵌入远端 Server 的 Mode Built-in Provider Config。 */
export function readBundledModeBuiltinProviderConfig(): string {
  if (typeof __MODE_BUILTIN_PROVIDER_CONFIG_JSON__ !== "string") {
    throw new Error("当前构建未嵌入 Mode Built-in Provider Config");
  }
  return __MODE_BUILTIN_PROVIDER_CONFIG_JSON__;
}

/**
 * 将 Mode Built-in Config 原子物化到所属环境的固定资源副本。
 * 升级前退出旧进程；不保留按内容 hash 增长的历史文件。
 */
export async function materializeBundledModeBuiltinProviderConfig(
  options: MaterializeBundledModeBuiltinProviderConfigOptions,
): Promise<string> {
  return materializeModeBuiltinProviderConfig(options);
}
