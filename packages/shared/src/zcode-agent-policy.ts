import { z } from "zod";
import type { CommandAgentSource } from "./command-types.js";
import type { ZCodeProvider } from "./zcode-task-types-core.js";

export const MODE_AGENT_PROVIDER = "glm" satisfies ZCodeProvider;
export const MODE_AGENT_PROVIDER_LABEL = "ZCode Agent";
export const MODE_COMMAND_AGENT_SOURCE = "zcodeAgent" satisfies CommandAgentSource;

export const zcodeAgentProviderSchema = z.literal(MODE_AGENT_PROVIDER);

export const MODE_COMMAND_AGENT_SOURCES = [
  MODE_COMMAND_AGENT_SOURCE,
] as const satisfies readonly CommandAgentSource[];

export function normalizeAgentProviderToZCodeAgent(
  _provider?: ZCodeProvider | null,
): ZCodeProvider {
  return MODE_AGENT_PROVIDER;
}

export function isZCodeAgentProvider(
  provider: ZCodeProvider | null | undefined,
): provider is typeof MODE_AGENT_PROVIDER {
  return provider === MODE_AGENT_PROVIDER;
}
