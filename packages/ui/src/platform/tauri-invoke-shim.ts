/**
 * Tauri `invoke()` 适配层（路线图 M1 Task 4 的 shim 部分）。
 *
 * 规格：`docs/specs/tauri-platform-commands.md`。命令名与 `IPlatformService` 逐字对齐，
 * Rust 侧 `crates/platform` 的切片表是另一端；两边任何一处改名，都会被
 * `crates/platform/tests/commands.rs` 的名字守卫或本文件的类型守卫暴露出来。
 *
 * ## 不实现的方法怎么处理（本层的关键约定）
 *
 * `IPlatformService` 里带 `?` 的可选方法由调用方用 `platform.foo?.()` 守卫，所以
 * **保持缺席**才是正确行为——给它们塞一个抛错的桩，会把「平台不支持就跳过」变成「直接崩」。
 *
 * 反之，**必选**方法调用方不会守卫。M1 没实现的必选方法必须显式抛
 * `not_implemented_until_m3`，让失败点带着方法名，而不是 `undefined is not a function`
 * 这种误导性错误。
 *
 * 覆盖关系由下面的 `_allRequiredMembersCovered` 在**编译期**校验：协议新增一条必选方法
 * 而本文件没跟上时，`pnpm typecheck` 会直接报错。
 */
import type {
  ApplicationIconRequest,
  IPlatformService,
  SaveFileRequest,
  SaveFileResult,
  TaskNotificationPayload,
} from "@mode/shared";

import { logger } from "../logger.js";

/** Tauri 的 `invoke`（由 `@tauri-apps/api/core` 提供）。这里只依赖这个形状，便于注入测试替身。 */
export type TauriInvoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>;

/** M1 切片表：与 `crates/platform/src/commands/mod.rs` 的 `command_slice!` 逐条对应。 */
export const M1_IMPLEMENTED_COMMANDS = [
  "openExternal",
  "selectDirectory",
  "selectFile",
  "saveFile",
  "openInFileManager",
  "showTaskNotification",
  "notifyRendererReady",
  "syncWindowTabs",
  "syncWindowUnreadCount",
  "getApplicationIcon",
] as const;

/** M1 未实现的**必选**方法：调用即抛 `not_implemented_until_m3`。 */
export const M1_UNIMPLEMENTED_REQUIRED_METHODS = [
  "onRemoteConnectionLog",
  "onRemoteSessionClosed",
  "onBotRemoteWorkspaceReconnected",
  "activateOrSetWorkspace",
  "connectRemote",
  "disposeRemoteSession",
  "isDockerAvailable",
  "getLanAccessState",
  "setLanAccessEnabled",
  "createLanAccessPairCode",
  "removeLanAccessClient",
  "resetLanAccessTokens",
  "discoverLanPeers",
  "pairLanPeer",
  "getLanPairedPeers",
  "getLanPeerWorkspaces",
  "removeLanPairedPeer",
  "renameLanPairedPeer",
  "listWSLDistros",
  "listDockerContainers",
  "listSSHConfigAliases",
  "openCommunity",
  "canOpenCommunity",
  "registerOAuthState",
  "onOAuthCallback",
  "onPaymentCallback",
  "syncActiveTaskSession",
  "onFocusTab",
  "onNewTab",
  "onNewTask",
  "onOpenWorkspace",
  "onWindowFullscreenChanged",
  "onTaskNotificationClick",
  "exportLogs",
  "onUpdateReady",
  "onUpdateCheckResult",
  "downloadUpdate",
  "cancelUpdateDownload",
  "skipUpdateVersion",
  "onPostUpdateReleaseNotes",
  "acknowledgePostUpdateReleaseNotes",
  "quitAndInstallUpdate",
  "getInstalledEditors",
  "openInEditor",
  "executeDesktopCommand",
  "setApplicationLocale",
  "setTitleBarTheme",
  "getDeviceId",
  "rdGetPeerCapability",
  "rdStartSession",
  "rdGetControlledState",
  "rdSendSignal",
  "rdEndSession",
  "rdReadClipboardText",
  "rdGetServerState",
  "rdSetServerEnabled",
  "rdRevokeClient",
  "rdAuthorizeClient",
  "onRemoteDesktopSessionEvent",
] as const;

type ImplementedCommand = (typeof M1_IMPLEMENTED_COMMANDS)[number];
type UnimplementedRequired = (typeof M1_UNIMPLEMENTED_REQUIRED_METHODS)[number];

/** `IPlatformService` 里没有 `?` 的成员名。 */
type RequiredMemberName = {
  [Key in keyof IPlatformService]-?: undefined extends IPlatformService[Key] ? never : Key;
}[keyof IPlatformService];

/** 既没实现、也没登记成「未实现必选」的必选成员。正常应为 `never`。 */
type UncoveredRequiredMember = Exclude<
  RequiredMemberName,
  ImplementedCommand | UnimplementedRequired
>;

// 编译期穷尽检查：不等于 never 时，这一行的类型不匹配会直接报错。
const _allRequiredMembersCovered: UncoveredRequiredMember extends never ? true : never = true;
void _allRequiredMembersCovered;

/** 未实现方法的统一错误文案；调用点靠它区分「骨架期缺件」与真实缺陷。 */
export function notImplementedUntilM3(method: string): Error {
  return new Error(`not_implemented_until_m3: ${method}`);
}

/**
 * 裸字符串形态的 `getApplicationIcon` 入参归一化。
 *
 * TS 侧签名是 `string | ApplicationIconRequest`，而 Rust 命令只收结构化 locator。
 * 字符串在该接口的历史语义里是「系统应用标识」，确切含义随平台而异（macOS 是 bundle id，
 * Windows 可能是 AUMID 或 exe 路径）。M1 不做平台猜测，把三种解释**都**交给 Rust 侧逐个尝试；
 * 真实图标解码已按 `docs/specs/tauri-platform-commands.md` §5 登记为 M3，当前结果恒为 null，
 * 所以这里只影响参数形状、不影响结果。真实解码落地时要回来确认字符串的语义。
 */
export function normalizeApplicationIconRequest(
  request: string | ApplicationIconRequest,
): ApplicationIconRequest {
  if (typeof request !== "string") return request;
  return {
    locators: [
      { kind: "windows-aumid", value: request },
      { kind: "windows-executable-path", value: request },
      { kind: "darwin-bundle-id", value: request },
    ],
  };
}

/**
 * 建一个只覆盖 M1 切片表的 `IPlatformService` 视图。
 *
 * 返回类型是 `Partial<IPlatformService>`：它**不是**完整实现，别当成 Electron 线的
 * `desktopPlatform` 用。
 */
export function createTauriPlatformService(invoke: TauriInvoke): Partial<IPlatformService> {
  const service: Partial<IPlatformService> = {
    openExternal(url: string): void {
      fireAndForget(invoke, "openExternal", { url });
    },
    selectDirectory(): Promise<string | null> {
      return invoke("selectDirectory") as Promise<string | null>;
    },
    selectFile(): Promise<string | null> {
      return invoke("selectFile") as Promise<string | null>;
    },
    saveFile(payload: SaveFileRequest): Promise<SaveFileResult> {
      return invoke("saveFile", { payload }) as Promise<SaveFileResult>;
    },
    openInFileManager(path: string): Promise<{ success: boolean; error?: string }> {
      return invoke("openInFileManager", { path }) as Promise<{
        success: boolean;
        error?: string;
      }>;
    },
    showTaskNotification(payload: TaskNotificationPayload): void {
      fireAndForget(invoke, "showTaskNotification", { payload });
    },
    notifyRendererReady(): void {
      fireAndForget(invoke, "notifyRendererReady");
    },
    syncWindowTabs(paths: string[]): void {
      fireAndForget(invoke, "syncWindowTabs", { paths });
    },
    syncWindowUnreadCount(count: number): void {
      fireAndForget(invoke, "syncWindowUnreadCount", { count });
    },
    getApplicationIcon(request: string | ApplicationIconRequest) {
      return invoke("getApplicationIcon", {
        request: normalizeApplicationIconRequest(request),
      }) as ReturnType<NonNullable<IPlatformService["getApplicationIcon"]>>;
    },
  };

  for (const method of M1_UNIMPLEMENTED_REQUIRED_METHODS) {
    (service as Record<string, unknown>)[method] = (..._args: unknown[]) => {
      throw notImplementedUntilM3(method);
    };
  }

  return service;
}

/**
 * 返回 `void` 的命令只能发后不管。**不能静默吞掉失败**——IPC 断了、命令没注册
 * （Rust 侧改名）都会在这里现形，所以统一记一条 warn 并把原始错误带上。
 */
function fireAndForget(
  invoke: TauriInvoke,
  command: string,
  args?: Record<string, unknown>,
): void {
  void invoke(command, args).catch((error: unknown) => {
    logger.warn(`[tauri-platform] ${command} 调用失败`, error);
  });
}
