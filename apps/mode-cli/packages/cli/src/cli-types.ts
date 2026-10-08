import type { TuiReadClipboardImage, TuiWriteClipboardText } from "@mode/tui";
import type { UiLocale } from "@mode/i18n";
import type { Logger } from "@mode/contracts";
import type {
  createManagedCdpBrowserRuntime,
  ManagedCdpBrowserRuntimeOptions,
} from "@mode/adapters/browser";
import type {
  createModelAdapter,
  createModeApp,
  CreateModelAdapterOptions,
  configureCodingPlanApiKey,
  ConfigureCodingPlanApiKeyOptions,
  inspectModeSkill,
  inspectWorkspaceHookTrust,
  grantWorkspaceHookTrust,
  revokeWorkspaceHookTrustCli,
  inspectModeCustomCommand,
  InspectModeCustomCommandOptions,
  InspectModeSkillOptions,
  loginModeCli,
  loginBigmodelCodingPlan,
  LoginBigmodelCodingPlanOptions,
  LoginModeCliOptions,
  listModeCustomCommands,
  ListModeCustomCommandsOptions,
  loadModeCustomCommand,
  listModeSessions,
  listModeSkills,
  ListModeSessionsOptions,
  ListModeSkillsOptions,
  logoutModeCli,
  LogoutModeCliOptions,
  resolveLatestSession,
  ResolveLatestSessionOptions,
  RunModeProtocolAgentOptions,
  startProcessProviderRegistryRuntime,
  ModeAppOptions,
} from "@mode/bootstrap";
import type { CliEnv, DotenvLoadResult, LoadCliDotenvOptions } from "./env.js";
import type { PluginsCommandOverrides } from "./plugins-command.js";
import type { CliShutdownProcess } from "./shutdown.js";
import type { resolveWorkspaceGitBranch } from "./tui-workspace-git.js";

export type BootstrapModule = typeof import("@mode/bootstrap");

export interface RunDependencies extends PluginsCommandOverrides {
  protocolLifecycle?: RunModeProtocolAgentOptions["lifecycle"];
  protocolInput?: NodeJS.ReadableStream;
  createManagedCdpBrowserRuntime?: (
    options?: ManagedCdpBrowserRuntimeOptions,
  ) => ReturnType<typeof createManagedCdpBrowserRuntime>;
  createModelAdapter?: (
    options?: CreateModelAdapterOptions,
  ) => ReturnType<typeof createModelAdapter>;
  createModeApp?: (
    options?: ModeAppOptions,
  ) => Awaited<ReturnType<typeof createModeApp>> | ReturnType<typeof createModeApp>;
  /**
   * Session-event shaper for --output-format stream-json. Defaults to the
   * bootstrap module's, which is also what the protocol server uses; injectable
   * so a caller that supplies its own `createModeApp` (tests, embedders) can
   * still stream, since the bootstrap module is not loaded on that path.
   */
  mapSessionEvent?: BootstrapModule["mapSessionEvent"];
  cwd?: () => string;
  env?: CliEnv;
  inspectSkill?: (options: InspectModeSkillOptions) => ReturnType<typeof inspectModeSkill>;
  inspectWorkspaceHookTrust?: typeof inspectWorkspaceHookTrust;
  grantWorkspaceHookTrust?: typeof grantWorkspaceHookTrust;
  revokeWorkspaceHookTrustCli?: typeof revokeWorkspaceHookTrustCli;
  inspectCustomCommand?: (
    options: InspectModeCustomCommandOptions,
  ) => ReturnType<typeof inspectModeCustomCommand>;
  loginModeCli?: (options?: LoginModeCliOptions) => ReturnType<typeof loginModeCli>;
  loginBigmodelCodingPlan?: (
    options?: LoginBigmodelCodingPlanOptions,
  ) => ReturnType<typeof loginBigmodelCodingPlan>;
  configureCodingPlanApiKey?: (
    options: ConfigureCodingPlanApiKeyOptions,
  ) => ReturnType<typeof configureCodingPlanApiKey>;
  loadDotenv?: (options?: LoadCliDotenvOptions) => DotenvLoadResult;
  projectConfigPath?: string;
  listSessions?: (options: ListModeSessionsOptions) => ReturnType<typeof listModeSessions>;
  listCustomCommands?: (
    options: ListModeCustomCommandsOptions,
  ) => ReturnType<typeof listModeCustomCommands>;
  loadCustomCommand?: (
    options: InspectModeCustomCommandOptions,
  ) => ReturnType<typeof loadModeCustomCommand>;
  // headless slash 路由要和 app facade 的保留名 gate 用同一个判据；默认取 bootstrap 的，
  // 注入点只为让单测不必拉起整个 bootstrap 模块。见 prompt-command.ts。
  isReservedSlashCommandName?: BootstrapModule["isReservedModeSlashCommandName"];
  listSkills?: (options: ListModeSkillsOptions) => ReturnType<typeof listModeSkills>;
  logger?: Logger;
  readClipboardImage?: TuiReadClipboardImage;
  writeClipboardText?: TuiWriteClipboardText;
  resolveLatestSession?: (
    options: ResolveLatestSessionOptions,
  ) => ReturnType<typeof resolveLatestSession>;
  resolveWorkspaceGitBranch?: typeof resolveWorkspaceGitBranch;
  logoutModeCli?: (options?: LogoutModeCliOptions) => ReturnType<typeof logoutModeCli>;
  runModeProtocolAgent?: (options?: RunModeProtocolAgentOptions) => Promise<void>;
  runTui?: typeof import("@mode/tui").runTui;
  skipUserConfig?: boolean;
  userConfigPath?: string;
  exitProcess?: (code: number) => void;
  shutdownCleanupTimeoutMs?: number;
  shutdownProcess?: CliShutdownProcess;
  startProcessProviderRegistryRuntime?: typeof startProcessProviderRegistryRuntime;
}

export type CliPermissionMode = "build" | "plan" | "edit" | "yolo";
export type CliRuntimeMode = CliPermissionMode | "auto";

export interface CliModeState {
  current?: CliRuntimeMode;
  override?: CliPermissionMode;
}

export interface CliTargetRequest {
  objective: string;
  replaceExisting: boolean;
}

export type ModeCapableApp = Awaited<ReturnType<typeof createModeApp>> & {
  getMode?: () => CliRuntimeMode;
  setLocale?: (locale: UiLocale) => Promise<{ locale: "en-US" | "zh-CN" | "fa-IR" }>;
  setMode?: (mode: CliRuntimeMode) => Promise<{ mode: CliRuntimeMode }>;
};

export interface CliResumeRequest {
  continueSession: boolean;
  resumeSessionId?: string;
}
