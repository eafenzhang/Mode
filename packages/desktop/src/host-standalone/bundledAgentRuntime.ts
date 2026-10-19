/**
 * 打包形态的 Agent 运行时定位（Host 进程内，Tauri/Node 宿主）。
 *
 * 背景：Electron 宿主用 `ELECTRON_RUN_AS_NODE` 执行 `resources/glm/mode.cjs`；
 * Tauri 宿主是纯 Node（`process.versions.electron` 不存在），services 的
 * `resolveDefaultModeAgentCommand` 既走不到 Electron 分支，也没有 monorepo 源码树
 * 与 `~/.mode/server/agents`，于是返回 null，发送任务报
 * "Mode agent server command is not configured"（2026-10-19 v0.0.57 本机验收实测）。
 *
 * 本模块只做一件事：若壳把**随包 agent** 放进了资源目录（`agent/glm/mode.cjs`），
 * 且调用方没有显式设置 `MODE_AGENT_SERVER_COMMAND`，就用当前 Host 的 Node
 * （打包形态即随包 `node/node.exe`）执行它。不覆盖显式配置，找不到文件则不动作
 * （开发形态继续走 monorepo 源码解析）。
 */
import { existsSync } from "node:fs";
import { join } from "node:path";

/** 随包 agent 入口相对资源目录的路径（与 `host-sidecar-ship.ts` 的落点一致）。 */
export const BUNDLED_AGENT_ENTRY_RELATIVE = join("agent", "glm", "mode.cjs");

export interface BundledAgentRuntime {
  command: string;
  argsJson: string;
  /** 随包 Built-in Provider Config；agent 进程需要它才能解析内置 provider。 */
  providerConfigPath?: string;
}

/**
 * 纯函数：按环境与文件存在性决定是否注入 agent 启动命令。
 * 返回 null 表示保持现状（显式配置优先，或没有随包 agent）。
 */
export function resolveBundledAgentRuntime(options: {
  env: NodeJS.ProcessEnv;
  execPath: string;
  exists: (path: string) => boolean;
}): BundledAgentRuntime | null {
  if (options.env.MODE_AGENT_SERVER_COMMAND?.trim()) return null;
  const resourcesDir = options.env.MODE_HOST_RESOURCES_DIR?.trim();
  if (!resourcesDir) return null;
  const entry = join(resourcesDir, BUNDLED_AGENT_ENTRY_RELATIVE);
  if (!options.exists(entry)) return null;
  const providerConfigPath = join(resourcesDir, "config", "provider", "mode-builtin.json");
  return {
    command: options.execPath,
    argsJson: JSON.stringify([entry, "app-server", "--stdio"]),
    ...(options.exists(providerConfigPath) && !options.env.MODE_BUILTIN_PROVIDER_CONFIG_FILE?.trim()
      ? { providerConfigPath }
      : {}),
  };
}

/** 把解析结果写回环境变量；返回是否发生注入。 */
export function applyBundledAgentRuntime(
  env: NodeJS.ProcessEnv = process.env,
  execPath: string = process.execPath,
  exists: (path: string) => boolean = existsSync,
): boolean {
  const runtime = resolveBundledAgentRuntime({ env, execPath, exists });
  if (!runtime) return false;
  env.MODE_AGENT_SERVER_COMMAND = runtime.command;
  env.MODE_AGENT_SERVER_ARGS_JSON = runtime.argsJson;
  if (runtime.providerConfigPath) {
    env.MODE_BUILTIN_PROVIDER_CONFIG_FILE = runtime.providerConfigPath;
  }
  return true;
}
