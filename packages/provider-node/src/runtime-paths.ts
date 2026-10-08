export const MODE_BUILTIN_PROVIDER_CONFIG_FILE_ENV = "MODE_BUILTIN_PROVIDER_CONFIG_FILE";
export const MODE_BUILTIN_PROVIDER_BUNDLED_CONFIG_FILE_ENV =
  "MODE_BUILTIN_PROVIDER_BUNDLED_CONFIG_FILE";
export const MODE_PERSONAL_PROVIDER_CONFIG_FILE_ENV = "MODE_PERSONAL_PROVIDER_CONFIG_FILE";
export const PERSONAL_PROVIDER_CONFIG_FILE_NAME = "provider_config.json";

export interface NodeProviderRuntimePaths {
  readonly modeBuiltinFilePath: string;
  readonly personalFilePath: string;
}

export function createNodeProviderRuntimePathEnv(
  paths: NodeProviderRuntimePaths,
): Record<string, string> {
  return {
    [MODE_BUILTIN_PROVIDER_CONFIG_FILE_ENV]: paths.modeBuiltinFilePath,
    [MODE_PERSONAL_PROVIDER_CONFIG_FILE_ENV]: paths.personalFilePath,
  };
}

export function resolveNodeProviderRuntimePaths(
  env: Readonly<Record<string, string | undefined>>,
): NodeProviderRuntimePaths | null {
  const modeBuiltinFilePath = env[MODE_BUILTIN_PROVIDER_CONFIG_FILE_ENV]?.trim();
  const personalFilePath = env[MODE_PERSONAL_PROVIDER_CONFIG_FILE_ENV]?.trim();
  if (!modeBuiltinFilePath && !personalFilePath) return null;
  if (!modeBuiltinFilePath || !personalFilePath) {
    throw new Error("Mode Built-in 与 Personal Provider Config 路径必须同时提供");
  }
  return Object.freeze({ modeBuiltinFilePath, personalFilePath });
}
