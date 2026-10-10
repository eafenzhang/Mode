// 电脑控制启用权威（docs/specs/computer-use-enablement.md）：
// 打包层 env kill-switch 与用户插件开关按 AND 组合。此前 node.ts 的 isCuaEnabledForContext
// 是 `env || 插件启用` 旁路——打包层 env 默认 ON 时，关闭开关仍会预热自研 Helper、注入
// broker 凭据，设置开关管不住自研链路。抽成独立谓词是为了让「kill-switch 压 stale config」
// 与「dev mode 不绕过插件启用态」两条语义有可执行断言（test/cua-enablement.test.ts）。
import { isModeCuaInternalFeatureEnabled } from "@mode/shared";
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
