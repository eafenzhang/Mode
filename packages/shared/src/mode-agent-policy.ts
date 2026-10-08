import { z } from "zod";
import type { CommandAgentSource } from "./command-types.js";
import type { ModeProvider } from "./mode-task-types-core.js";

export const MODE_AGENT_PROVIDER = "glm" satisfies ModeProvider;
export const MODE_AGENT_PROVIDER_LABEL = "Mode Agent";
export const MODE_COMMAND_AGENT_SOURCE = "modeAgent" satisfies CommandAgentSource;

export const modeAgentProviderSchema = z.literal(MODE_AGENT_PROVIDER);

export const MODE_COMMAND_AGENT_SOURCES = [
  MODE_COMMAND_AGENT_SOURCE,
] as const satisfies readonly CommandAgentSource[];

export function normalizeAgentProviderToModeAgent(
  _provider?: ModeProvider | null,
): ModeProvider {
  return MODE_AGENT_PROVIDER;
}

export function isModeAgentProvider(
  provider: ModeProvider | null | undefined,
): provider is typeof MODE_AGENT_PROVIDER {
  return provider === MODE_AGENT_PROVIDER;
}
