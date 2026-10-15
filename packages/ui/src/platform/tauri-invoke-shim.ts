/**
 * Tauri `invoke()` 适配层（路线图 M1 Task 4 的 shim 部分 + Task 6 的事件通道）。
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
 *
 * ## 事件通道（M1 唯一一条：深链回调）
 *
 * `onOAuthCallback` / `onPaymentCallback` 订阅的是宿主 emit 的同一个深链事件
 * [`DEEP_LINK_EVENT`]，按载荷里的 `kind` 分流。事件名、载荷类型与 `kind` 取值全部来自
 * `crates/protocol`（生成物），不手抄——事件名写错是**静默失效**（订阅装上了却永远收不到）。
 */
import type {
  ApplicationIconRequest,
  DeepLinkEvent,
  DeepLinkEventKind,
  IPlatformService,
  SaveFileRequest,
  SaveFileResult,
  TaskNotificationPayload,
} from "@mode/shared";
import { DEEP_LINK_EVENT } from "@mode/shared";

import { logger } from "../logger.js";

/** Tauri 的 `invoke`（由 `@tauri-apps/api/core` 提供）。这里只依赖这个形状，便于注入测试替身。 */
export type TauriInvoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>;

/** 退订句柄，与 `@tauri-apps/api/event` 的 `UnlistenFn` 同形。 */
export type TauriUnlisten = () => void;

/** 事件载荷信封，与 `@tauri-apps/api/event` 的 `Event<T>` 同形。 */
export type TauriEventEnvelope<T> = { event: string; payload: T };

/** Tauri 的 `listen`。同样只依赖形状，注入测试替身用。 */
export type TauriListen = <T>(
  event: string,
  handler: (envelope: TauriEventEnvelope<T>) => void,
) => Promise<TauriUnlisten>;

/** 深链回调映射到事件载荷的 `kind` 值。取值来自生成物，不手抄。 */
const DEEP_LINK_CALLBACK_KINDS = {
  onOAuthCallback: "oauth-callback",
  onPaymentCallback: "payment-callback",
} as const satisfies Record<string, DeepLinkEventKind>;

// 编译期守卫：上面两个映射值必须是 DeepLinkEventKind 的成员（`satisfies` 已覆盖），
// 且必须覆盖本层实现的两条订阅方法。
type DeepLinkCallbackMethod = keyof typeof DEEP_LINK_CALLBACK_KINDS;
const _deepLinkCallbackMethods: readonly DeepLinkCallbackMethod[] = [
  "onOAuthCallback",
  "onPaymentCallback",
];
void _deepLinkCallbackMethods;

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

/**
 * M1 通过**事件通道**实现（不是命令，见 spec §2）的方法。
 *
 * 单列一份而不是直接混进 `M1_IMPLEMENTED_COMMANDS`：那张表是「Rust 侧有同名
 * `#[tauri::command]`」的清单，由 `crates/platform/tests/commands.rs` 对拍；
 * 这两条方法走的是宿主单向 emit 的事件，Rust 侧没有对应命令，混进去会让那张表的
 * 语义变脏、也过不了跨端名字守卫。
 */
export const M1_EVENT_SUBSCRIBED_METHODS = ["onOAuthCallback", "onPaymentCallback"] as const;

type ImplementedCommand = (typeof M1_IMPLEMENTED_COMMANDS)[number];
type EventSubscribedCommand = (typeof M1_EVENT_SUBSCRIBED_METHODS)[number];
type UnimplementedRequired = (typeof M1_UNIMPLEMENTED_REQUIRED_METHODS)[number];

/** `IPlatformService` 里没有 `?` 的成员名。 */
type RequiredMemberName = {
  [Key in keyof IPlatformService]-?: undefined extends IPlatformService[Key] ? never : Key;
}[keyof IPlatformService];

/** 既没实现、也没登记成「未实现必选」的必选成员。正常应为 `never`。 */
type UncoveredRequiredMember = Exclude<
  RequiredMemberName,
  ImplementedCommand | EventSubscribedCommand | UnimplementedRequired
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
 * 建一个只覆盖 M1 切片表（命令面 + 一条深链事件通道）的 `IPlatformService` 视图。
 *
 * 返回类型是 `Partial<IPlatformService>`：它**不是**完整实现，别当成 Electron 线的
 * `desktopPlatform` 用。
 *
 * `listen` 是必填参数（不是可选的第二参数）：`onOAuthCallback` 无法在没有订阅能力时
 * 成立，允许省略就会退化成「订阅装不上但调用方以为装上了」——那正是本文件开头
 * 反对的假值。省略它在 TS 层就是编译错误。
 */
export function createTauriPlatformService(
  invoke: TauriInvoke,
  listen: TauriListen,
): Partial<IPlatformService> {
  /**
   * 尚未装好的订阅。`notifyRendererReady` 必须等它们全部完成再握手（见下）。
   */
  const pendingSubscriptions: Promise<void>[] = [];

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
      // **顺序是语义，不是实现细节**：宿主收到这次握手就会立刻补发积压的冷启动深链
      // （`crates/platform/src/commands/window.rs` 的 notify_renderer_ready）。
      // 订阅还没生效就握手，补发的事件没人接——正好把握手要解决的问题又制造一遍。
      // 所以先等全部订阅就位，再发握手。
      void Promise.all(pendingSubscriptions)
        .then(() => invoke("notifyRendererReady"))
        .catch((error: unknown) => {
          logger.warn("[tauri-platform] notifyRendererReady 调用失败", error);
        });
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
    onOAuthCallback(callback: (url: string) => void): () => void {
      const unsubscribe = subscribeDeepLink(
        listen,
        DEEP_LINK_CALLBACK_KINDS.onOAuthCallback,
        callback,
      );
      pendingSubscriptions.push(unsubscribe.ready);
      return unsubscribe.dispose;
    },
    onPaymentCallback(callback: (url: string) => void): () => void {
      const unsubscribe = subscribeDeepLink(
        listen,
        DEEP_LINK_CALLBACK_KINDS.onPaymentCallback,
        callback,
      );
      pendingSubscriptions.push(unsubscribe.ready);
      return unsubscribe.dispose;
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
 * 订阅深链事件里某一类 `kind`。
 *
 * 返回同步的 `dispose`（接口契约要求退订函数是同步的）与异步的 `ready`
 * （`listen` 本身是异步的，握手前要等它）。
 *
 * 退订与「订阅还没装好就退订」的竞态：`dispose` 先置位，`listen` 兑现时若发现已退订
 * 就立刻自己退订，不留悬挂监听。
 */
function subscribeDeepLink(
  listen: TauriListen,
  kind: DeepLinkEventKind,
  callback: (url: string) => void,
): { dispose: () => void; ready: Promise<void> } {
  let disposed = false;
  let unlisten: TauriUnlisten | null = null;

  const ready = listen<DeepLinkEvent>(DEEP_LINK_EVENT, (envelope) => {
    const payload = envelope?.payload;
    // 载荷来自 IPC，不能假设它符合类型：形状不对时**跳过而不是抛**，
    // 一个畸形事件不该打断整条订阅。
    if (!payload || typeof payload !== "object") return;
    if (payload.kind !== kind) return;
    if (typeof payload.url !== "string" || payload.url.length === 0) return;
    callback(payload.url);
  }).then(
    (stop) => {
      if (disposed) {
        stop();
        return;
      }
      unlisten = stop;
    },
    (error: unknown) => {
      // 订阅失败必须能被看到：否则表现是「OAuth 回调永远不来」，与「用户没完成登录」
      // 无法区分。这里不重试——重试策略属 M3 的连接管理层。
      logger.warn(`[tauri-platform] 订阅 ${DEEP_LINK_EVENT}（${kind}）失败`, error);
    },
  );

  return {
    dispose: () => {
      disposed = true;
      unlisten?.();
      unlisten = null;
    },
    ready,
  };
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
