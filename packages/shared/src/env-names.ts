/**
 * 对外契约环境变量改名的兼容层。
 *
 * 改名分两步走到 Mode：`ZCODE_*`（上游名）→ `ZCODIUM_*`（P1a）→ `MODE_*`（P2，本次）。
 * - 读取端按 `MODE_*` → `ZCODIUM_*` → `ZCODE_*` 逐级回退，读到旧名时一次性弃用提示；
 * - 进程内给子进程传值时三代名字全部写上，覆盖新旧二进制混布（SSH 远端旧 agent 只认旧名）。
 *
 * 名单见 docs/specs/p1a-external-env-renames.md 与 docs/specs/p2-mode-naming.md；
 * 仅覆盖用户/CI/文档/安装器真实会设置的对外变量，进程间内部变量不做兼容。
 */

/** 三代名字表：key = 当前名（MODE_*），value = 逐级兜底的旧名（由近到远）。 */
export const EXTERNAL_ENV_LEGACY_ALIASES: Readonly<Record<string, readonly string[]>> = {
  MODE_DATA_BASE_DIR: ["ZCODIUM_DATA_BASE_DIR", "ZCODE_DATA_BASE_DIR"],
  MODE_BASE_URL: ["ZCODIUM_BASE_URL", "ZCODE_BASE_URL"],
  MODE_CDN_BASE_URL: ["ZCODIUM_CDN_BASE_URL", "ZCODE_CDN_BASE_URL"],
  MODE_DEPS_BASE_URL: ["ZCODIUM_DEPS_BASE_URL", "ZCODE_DEPS_BASE_URL"],
  MODE_DIST_BASE_URL: ["ZCODIUM_DIST_BASE_URL", "ZCODE_DIST_BASE_URL"],
  MODE_REMOTE_ASSET_CDN_BASE_URL: [
    "ZCODIUM_REMOTE_ASSET_CDN_BASE_URL",
    "ZCODE_REMOTE_ASSET_CDN_BASE_URL",
  ],
  MODE_CONVERSATION_SHARE_WEB_URL: [
    "ZCODIUM_CONVERSATION_SHARE_WEB_URL",
    "ZCODE_CONVERSATION_SHARE_WEB_URL",
  ],
  MODE_BUILTIN_PROVIDER_CONFIG_FILE: [
    "ZCODIUM_BUILTIN_PROVIDER_CONFIG_FILE",
    "ZCODE_BUILTIN_PROVIDER_CONFIG_FILE",
  ],
  MODE_BUILTIN_PROVIDER_BUNDLED_CONFIG_FILE: [
    "ZCODIUM_BUILTIN_PROVIDER_BUNDLED_CONFIG_FILE",
    "ZCODE_BUILTIN_PROVIDER_BUNDLED_CONFIG_FILE",
  ],
  MODE_PERSONAL_PROVIDER_CONFIG_FILE: [
    "ZCODIUM_PERSONAL_PROVIDER_CONFIG_FILE",
    "ZCODE_PERSONAL_PROVIDER_CONFIG_FILE",
  ],
  MODE_STORAGE_DIR: ["ZCODIUM_STORAGE_DIR", "ZCODE_STORAGE_DIR"],
  MODE_SERVER_WORKSPACE: ["ZCODIUM_SERVER_WORKSPACE", "ZCODE_SERVER_WORKSPACE"],
  MODE_SERVER_AUTH_TOKEN: ["ZCODIUM_SERVER_AUTH_TOKEN", "ZCODE_SERVER_AUTH_TOKEN"],
  MODE_PROJECT_DIR: ["ZCODIUM_PROJECT_DIR", "ZCODE_PROJECT_DIR"],
  MODE_DIST_HOME: ["ZCODIUM_DIST_HOME", "ZCODE_DIST_HOME"],
  MODE_DIST_BIN_DIR: ["ZCODIUM_DIST_BIN_DIR", "ZCODE_DIST_BIN_DIR"],
  MODE_PLUGIN_ROOT: ["ZCODIUM_PLUGIN_ROOT", "ZCODE_PLUGIN_ROOT"],
  MODE_PLUGIN_DATA: ["ZCODIUM_PLUGIN_DATA", "ZCODE_PLUGIN_DATA"],
  // P1a 名单之外，但对用户/CI 同样可见：环境选择与数据根策略。
  MODE_ENV: ["ZCODIUM_ENV", "ZCODE_ENV"],
  MODE_HOME: ["ZCODIUM_HOME", "ZCODE_HOME"],
  MODE_DATA_ROOT_ACTION: ["ZCODIUM_DATA_ROOT_ACTION", "ZCODE_DATA_ROOT_ACTION"],
  MODE_APP_VERSION: ["ZCODIUM_APP_VERSION", "ZCODE_APP_VERSION"],
} as const;

/** 该名字是否有旧名兜底（用于给子进程传值时决定要不要写旧名）。 */
export function externalEnvLegacyAliases(name: string): readonly string[] {
  return EXTERNAL_ENV_LEGACY_ALIASES[name] ?? [];
}

/** 读改名变量：当前名优先，旧名由近到远兜底（弃用兼容期）。 */
export function readExternalEnvVar(
  env: Record<string, string | undefined>,
  name: string,
): string | undefined {
  const primary = env[name]?.trim();
  if (primary) return primary;
  for (const legacyName of externalEnvLegacyAliases(name)) {
    const legacyValue = env[legacyName]?.trim();
    if (legacyValue) {
      emitLegacyEnvDeprecation(legacyName, name);
      return legacyValue;
    }
  }
  return undefined;
}

/** 三代名字全部写入：覆盖新旧二进制混布（如 SSH 远端旧 agent 仍读旧名）。 */
export function writeExternalEnvVar(
  env: Record<string, string | undefined>,
  name: string,
  value: string | undefined,
): void {
  const names = [name, ...externalEnvLegacyAliases(name)];
  if (value === undefined) {
    for (const key of names) delete env[key];
    return;
  }
  for (const key of names) env[key] = value;
}

const warnedLegacyEnvKeys = new Set<string>();

export function emitLegacyEnvDeprecation(legacyName: string, currentName: string): void {
  if (warnedLegacyEnvKeys.has(legacyName)) return;
  warnedLegacyEnvKeys.add(legacyName);
  // 无统一 logger 可用的早期路径（bootstrap 前）也允许输出；进程级去重。
  console.warn(
    `[env] 环境变量 ${legacyName} 已更名为 ${currentName}，旧名本版本仍兼容，请尽快迁移`,
  );
}
