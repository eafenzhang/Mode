// 电脑控制启用权威（docs/specs/computer-use-enablement.md）：
// 打包层 env kill-switch 与用户插件开关按 AND 组合。此前 node.ts 的 isCuaEnabledForContext
// 是 `env || 插件启用` 旁路——打包层 env 默认 ON 时，关闭开关仍会预热自研 Helper、注入
// broker 凭据，设置开关管不住自研链路。抽成独立谓词是为了让「kill-switch 压 stale config」
// 与「dev mode 不绕过插件启用态」两条语义有可执行断言（test/cua-enablement.test.ts）。
import {
  isModeCuaInternalFeatureEnabled,
  isRemoteWorkspaceIdentity,
  parseRemoteWorkspaceIdentity,
} from "@mode/shared";
import { isOfficialCuaPluginEnabledForWorkspace } from "#src/cua-permission-broker/index.js";

export function resolveCuaWorkspaceEnablement(
  input: {
    env?: NodeJS.ProcessEnv;
    /** project 层插件配置的发现起点；缺省与消费方一致走 process.cwd() 语义。 */
    workingDirectory?: string;
  } = {},
): boolean {
  const env = input.env ?? process.env;
  // 打包层 kill-switch：只能关、不能开。env 关闭时即使配置残留 true 也全链路压住，
  // 否则 MODE_CUA_PRODUCT_HELPER=0 的构建会被旧配置重新点着。
  if (!isModeCuaInternalFeatureEnabled(env)) return false;
  // 用户权威门：设置页「电脑控制」与插件页写入的插件启用态（缺省为关）。
  return isOfficialCuaPluginEnabledForWorkspace({
    env,
    workingDirectory: input.workingDirectory,
  });
}

/**
 * CUA helper admission 的 remote identity 排除判定（唯一口径，见 docs/specs/lan-remote-computer-use.md）：
 * - 空 identity：非 remote，不排除；
 * - remote:lan：对端是完整 desktop 自己（desktop-local authority），helper 落在对端本机
 *   操作对端自己的屏幕，放行；
 * - ssh/wsl/docker：对端没有 desktop/Helper，排除；
 * - 无法解析的非空串：保持历史「有 identity 即挡」语义，排除（fail closed）。
 */
export function isCuaExcludedRemoteWorkspaceIdentity(identity?: string | null): boolean {
  const trimmed = identity?.trim();
  if (!trimmed) return false;
  if (!isRemoteWorkspaceIdentity(trimmed)) return true;
  return parseRemoteWorkspaceIdentity(trimmed)?.kind !== "lan";
}
