import { z } from "zod";
import type { CommandAgentSource } from "./command-types.js";
import type { ModeProvider } from "./mode-task-types-core.js";

export const MODE_AGENT_PROVIDER = "glm" satisfies ModeProvider;
export const MODE_AGENT_PROVIDER_LABEL = "Mode Agent";
export const MODE_COMMAND_AGENT_SOURCE = "modeAgent" satisfies CommandAgentSource;

/**
 * 解析持久化/relay 数据时的 provider 归一 schema：
 * 已移除的第三方 provider 历史值仅作为归一化输入保留，统一收敛为 glm；
 * 其它非法值保持拒绝，与收窄前的校验边界一致。
 */
export const legacyModeProviderSchema = z
  .enum(["claude", "opencode", "gemini", "codex", "glm"])
  .transform((): typeof MODE_AGENT_PROVIDER => MODE_AGENT_PROVIDER);

export const MODE_ENABLED_AGENT_PROVIDERS = [
  MODE_AGENT_PROVIDER,
] as const satisfies readonly ModeProvider[];

export const MODE_COMMAND_AGENT_SOURCES = [
  MODE_COMMAND_AGENT_SOURCE,
] as const satisfies readonly CommandAgentSource[];

export function normalizeAgentProviderToModeAgent(
  _provider?: ModeProvider | null,
): ModeProvider {
  return MODE_AGENT_PROVIDER;
}

export function normalizeAgentProvidersToModeAgent(
  _providers?: ReadonlyArray<ModeProvider> | null,
): ModeProvider[] {
  return [...MODE_ENABLED_AGENT_PROVIDERS];
}

export function normalizeCommandAgentSourceToModeAgent(
  _agentSource?: CommandAgentSource | null,
): CommandAgentSource {
  return MODE_COMMAND_AGENT_SOURCE;
}

export function isModeAgentProvider(
  provider: ModeProvider | null | undefined,
): provider is typeof MODE_AGENT_PROVIDER {
  return provider === MODE_AGENT_PROVIDER;
}
