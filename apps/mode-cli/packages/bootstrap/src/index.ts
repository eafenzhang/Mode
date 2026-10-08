// Bootstrap public API surface.

export * from "./app/create-app.js";
export type {
  ListModeSessionsOptions,
  PromptInput,
  ResolveLatestSessionOptions,
  ResumeOptions,
  RunModeProtocolAgentOptions,
  SendInputOptions,
  SendInputResult,
  SetLocaleResult,
  SteerTurnOptions,
  SubmitPromptOptions,
  UserPromptInput,
  ModeApp,
  ModeAppOptions,
  ModeModelOption,
} from "./app/types.js";
export * from "./auth-login.js";
export {
  inspectModeCustomCommand,
  listModeCustomCommands,
  loadModeCustomCommand,
} from "./custom-commands.js";
export type {
  InspectModeCustomCommandOptions,
  ListModeCustomCommandsOptions,
  ModeCustomCommandInspection,
} from "./custom-commands.js";
export { createModelAdapter } from "./model-factory.js";
export type { CreateModelAdapterOptions } from "./model-factory.js";
export { startProcessProviderRegistryRuntime } from "./app/process-provider-registry-runtime.js";
export type { ProcessProviderRegistryRuntimeOptions } from "./app/process-provider-registry-runtime.js";
export {
  addModePluginMarketplace,
  getModePluginsOverview,
  installModeMarketplacePlugin,
  listModePlugins,
  removeModePluginMarketplace,
  resolveModePlugins,
  setModePluginEnabled,
  uninstallModeMarketplacePlugin,
  updateModeMarketplacePlugin,
  updateModePluginMarketplace,
  validateModePluginPath,
} from "./plugins.js";
export type {
  AddModeMarketplaceOptions,
  InstallModeMarketplacePluginOptions,
  ListModePluginsOptions,
  RemoveModeMarketplaceOptions,
  ResolveModePluginsOptions,
  SetModePluginEnabledOptions,
  SetModePluginEnabledResult,
  UninstallModeMarketplacePluginOptions,
  UpdateModeMarketplaceOptions,
  UpdateModeMarketplacePluginOptions,
  ValidateModePluginPathOptions,
  ModeAvailablePluginData,
  ModeInstalledPluginData,
  ModeMarketplaceSummaryData,
  ModeMarketplaceUpdateData,
  ModePluginInstallData,
  ModePluginUpdateData,
  ModePluginsOverviewData,
} from "./plugins.js";
export { runModeProtocolAgent } from "./mode-protocol-entrypoint.js";
// Exposed for the CLI's --output-format stream-json: it needs the same event
// shape the protocol server emits, rather than inventing a second one.
export { mapSessionEvent } from "./mode-protocol/session-mapper.js";
export type { SessionTranscriptMessage, SessionTranscriptPart } from "./session-transcript.js";
export { listModeSessions, resolveLatestSession } from "./sessions.js";
export { inspectModeSkill, listModeSkills } from "./skills.js";
export type {
  InspectModeSkillOptions,
  ListModeSkillsOptions,
  ModeSkillInspection,
} from "./skills.js";
// Exposed for the CLI's headless slash routing: it must decide "is this a real
// custom command?" with the *same* reserved-name gate the app facade's
// customCommandPromptResolver applies, or the two disagree and a reserved name
// reaches the model as literal prompt text. See prompt-command.ts.
export { isReservedModeSlashCommandName } from "./slash-command-surface.js";
export {
  grantWorkspaceHookTrust,
  inspectWorkspaceHookTrust,
  revokeWorkspaceHookTrustCli,
} from "./workspace-hook-trust-cli.js";
export type {
  WorkspaceHookTrustCliItem,
  WorkspaceHookTrustCliStatus,
  WorkspaceHookTrustCliTarget,
} from "./workspace-hook-trust-cli.js";
