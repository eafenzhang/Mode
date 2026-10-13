import {
  MODE_AGENT_PROVIDER,
  MODE_COMMAND_AGENT_SOURCES,
  type ModeProvider,
  type CommandAgentSource,
} from "@mode/shared";

export const COMMAND_AGENT_SOURCES = [
  ...MODE_COMMAND_AGENT_SOURCES,
] as const satisfies readonly CommandAgentSource[];

export const COMMAND_AGENT_SOURCE_META: Record<
  CommandAgentSource,
  {
    labelId: string;
    provider: ModeProvider;
    pathHint: string;
  }
> = {
  modeAgent: {
    labelId: "settings.commands.source.modeAgent",
    pathHint: "~/.mode/commands",
    provider: MODE_AGENT_PROVIDER,
  },
};
