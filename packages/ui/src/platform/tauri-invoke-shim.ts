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
  DesktopCommandId,
  DesktopWindowChromeState,
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

/**
 * 窗口控制面的 wire 名。
 *
 * 与注入层同源、但**不能共享一个常量**：注入层的名字由壳用占位符注入
 * （`crates/sidecar-host/src/bridge.rs` 是真源），而这一层是 TS 侧的另一条适配路径
 * （开发用的验收台页），它拿不到那段注入。
 *
 * 三处字面量的一致性由 `windowModeInjection.test.mts` 的
 * 「壳侧窗口控制面的 rename 字面量必须与注入名一致」守：它把
 * `apps/desktop-tauri/src-tauri/src/window_controls.rs` 的 `rename` 字面量、`bridge.rs` 的
 * `CMD_*` 常量、以及本文件的这两个常量**逐个对拍**（任一处漂移即红）。
 */
const EXECUTE_DESKTOP_COMMAND = "executeDesktopCommand";
const GET_WINDOW_CHROME_STATE = "getDesktopWindowChromeState";

/** 深链回调映射到事件载荷的 `kind` 值。取值来自生成物，不手抄。 */
const DEEP_LINK_CALLBACK_KINDS = {
  onOAuthCallback: "oauth-callback",
  onPaymentCallback: "payment-callback",
  onShareImport: "share-import",
  onOpenWorkspacePath: "workspace-open",
} as const satisfies Record<string, DeepLinkEventKind>;

/**
 * 编译期守卫（**两个方向，缺一不可**；措辞按构造能真正保证的东西写）：
 *
 * - `_everySubscribedMethodHasAKind` 拒绝**多**：`EVENT_SUBSCRIBED_METHODS` 里出现一个不在
 *   `DEEP_LINK_CALLBACK_KINDS` 里的名字 → 类型不匹配，`tsc` 红；
 * - `_everyKindHasASubscribedMethod` 拒绝**漏**：往 `DEEP_LINK_CALLBACK_KINDS` 里加一个 `kind`
 *   却没有对应的订阅方法 → `Exclude<…>` 不再是 `never`，同样在编译期红。
 *
 * （早先这里只有前一个方向、却写成「漏一条这个类型就不成立」——**那句话与构造不符**，
 * 已改正。① 本轮还发现：这段守卫的一次修改因为替换文本对不上而**静默没生效**，
 * 是靠「故意加一个 kind，看 tsc 会不会红」的 RED 检查才抓出来的——守卫本身也要被验。）
 */
type DeepLinkCallbackMethod = keyof typeof DEEP_LINK_CALLBACK_KINDS;

const _everySubscribedMethodHasAKind: readonly DeepLinkCallbackMethod[] = [
  "onOAuthCallback",
  "onPaymentCallback",
  "onShareImport",
  "onOpenWorkspacePath",
];
void _everySubscribedMethodHasAKind;

/**
 * 映射里有、但 `EVENT_SUBSCRIBED_METHODS` 里没有的成员（正常应为 `never`）。
 *
 * 第二个类型参数是**订阅方法清单**（`EventSubscribedCommand`），不是映射的键：
 * 写成 `Exclude<键, 键>` 恒等于 `never`，那种「守卫」永远不会红（本文件写过一次，
 * 被自证的 RED 检查抓出来）。
 */
type UnsubscribedDeepLinkKind = Exclude<
  keyof typeof DEEP_LINK_CALLBACK_KINDS,
  EventSubscribedCommand
>;

const _everyKindHasASubscribedMethod: UnsubscribedDeepLinkKind extends never ? true : never = true;
void _everyKindHasASubscribedMethod;

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
 * 通过**宿主的深链事件通道**实现（不是命令，见 spec §2）的方法。
 *
 * 单列一份而不是直接混进 `M1_IMPLEMENTED_COMMANDS`：那张表是「Rust 侧有同名
 * `#[tauri::command]`」的清单，由 `crates/platform/tests/commands.rs` 对拍；
 * 这些方法走的是宿主单向 emit 的事件，Rust 侧没有对应命令，混进去会让那张表的
 * 语义变脏、也过不了跨端名字守卫。
 *
 * 名单演进：M1 落 `onOAuthCallback` / `onPaymentCallback`（spec §2 的「只落一个示范」是下限，
 * 支付走同一条链路所以一并接）；**M3 Task 4 把剩下两条也接上**——`onShareImport` 与
 * `onOpenWorkspacePath` 在接口声明里就是**深链驱动**的（见 `packages/shared/src/platform.ts`
 * 的文档注释：`mode://share/import?code=…` / 「通过 deep link 直接打开本地工作区目录」），
 * 而 `share-import` / `workspace-open` 两个 `kind` 从 M1 起就由路由投递，只是一直没有订阅点。
 * 于是「宿主认得的五种深链」在本层**全部有落点**，没有「投了但没人接」的种类。
 */
export const EVENT_SUBSCRIBED_METHODS = [
  "onOAuthCallback",
  "onPaymentCallback",
  "onShareImport",
  "onOpenWorkspacePath",
] as const;

/**
 * **窗口控制面**（M3 Task 4 fix round）：产品 UI 的自绘标题栏调用的成员。
 *
 * 单列一份的理由与 `EVENT_SUBSCRIBED_METHODS` 相同（它们既不是 M1 切片里的命令、也不是事件订阅的
 * 深链面），但更要紧的是**清单本身就是守卫**：`windowModeInjection.test.mts` 会断言这几个成员
 * 在注入的 `window.mode` 上真的存在——将来谁把它们撤掉（或又退回「降级空操作」），
 * 那条断言就红。用户报的缺陷正是「它们在 Tauri 壳里是死的」（点标题栏没反应）。
 */
export const WINDOW_CONTROL_METHODS = [
  "executeDesktopCommand",
  "getDesktopWindowChromeState",
  "onDesktopWindowChromeStateChanged",
] as const;

type ImplementedCommand = (typeof M1_IMPLEMENTED_COMMANDS)[number];
type EventSubscribedCommand = (typeof EVENT_SUBSCRIBED_METHODS)[number];
type WindowControlCommand = (typeof WINDOW_CONTROL_METHODS)[number];
type UnimplementedRequired = (typeof M1_UNIMPLEMENTED_REQUIRED_METHODS)[number];

/** `IPlatformService` 里没有 `?` 的成员名。 */
type RequiredMemberName = {
  [Key in keyof IPlatformService]-?: undefined extends IPlatformService[Key] ? never : Key;
}[keyof IPlatformService];

/** 既没实现、也没登记成「未实现必选」的必选成员。正常应为 `never`。 */
type UncoveredRequiredMember = Exclude<
  RequiredMemberName,
  ImplementedCommand | EventSubscribedCommand | WindowControlCommand | UnimplementedRequired
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
        urlFromPayload,
        callback,
      );
      pendingSubscriptions.push(unsubscribe.ready);
      return unsubscribe.dispose;
    },
    onPaymentCallback(callback: (url: string) => void): () => void {
      const unsubscribe = subscribeDeepLink(
        listen,
        DEEP_LINK_CALLBACK_KINDS.onPaymentCallback,
        urlFromPayload,
        callback,
      );
      pendingSubscriptions.push(unsubscribe.ready);
      return unsubscribe.dispose;
    },
    onShareImport(callback: (payload: { shareCode: string }) => void): () => void {
      const unsubscribe = subscribeDeepLink(
        listen,
        DEEP_LINK_CALLBACK_KINDS.onShareImport,
        (payload) =>
          typeof payload.code === "string" && payload.code.length > 0
            ? { shareCode: payload.code }
            : undefined,
        callback,
      );
      pendingSubscriptions.push(unsubscribe.ready);
      return unsubscribe.dispose;
    },
    onOpenWorkspacePath(callback: (path: string) => void): () => void {
      const unsubscribe = subscribeDeepLink(
        listen,
        DEEP_LINK_CALLBACK_KINDS.onOpenWorkspacePath,
        pathFromPayload,
        callback,
      );
      pendingSubscriptions.push(unsubscribe.ready);
      return unsubscribe.dispose;
    },
    // ---- 窗口控制面（自绘标题栏）------------------------------------------------
    // 三个按钮分别发 `minimizeWindow` / `toggleMaximizeWindow` / `closeWindow`，挂载时读一次
    // chrome 状态决定画「最大化」还是「还原」图标。命令 id 的取值集合在 `@mode/shared` 的
    // `DesktopCommandIds`；壳只实现窗口级的那些，其余 id 带回 `{ success: false, error }`。
    executeDesktopCommand(command: DesktopCommandId): Promise<unknown> {
      return invoke(EXECUTE_DESKTOP_COMMAND, { command });
    },
    getDesktopWindowChromeState(): Promise<DesktopWindowChromeState> {
      return invoke(GET_WINDOW_CHROME_STATE) as Promise<DesktopWindowChromeState>;
    },
    onDesktopWindowChromeStateChanged(
      handler: (state: DesktopWindowChromeState) => void,
    ): () => void {
      return subscribeWindowChromeState(invoke, handler);
    },
  };

  for (const method of M1_UNIMPLEMENTED_REQUIRED_METHODS) {
    (service as Record<string, unknown>)[method] = (..._args: unknown[]) => {
      throw notImplementedUntilM3(method);
    };
  }

  return service;
}

/** 深链载荷里的一种「要交给订阅方的东西」；投影不出来（缺字段/形状不对）就是 `undefined`。 */
type DeepLinkProjection<T> = (payload: DeepLinkEvent) => T | undefined;

/** `url` 投影（oauth / payment 用）；形状不对返回 `undefined` 而不是抛。 */
function urlFromPayload(payload: DeepLinkEvent): string | undefined {
  return typeof payload.url === "string" && payload.url.length > 0 ? payload.url : undefined;
}

/** `path` 投影（`workspace-open` 用）。 */
function pathFromPayload(payload: DeepLinkEvent): string | undefined {
  return typeof payload.path === "string" && payload.path.length > 0 ? payload.path : undefined;
}

/**
 * 订阅窗口 chrome 状态变化（`onDesktopWindowChromeStateChanged`）。
 *
 * 与注入层同一套做法：用 Tauri **内置**的窗口 `onResized` 做触发点（最大化/还原/全屏都会改尺寸），
 * 去抖后重读状态，只在 `isMaximized` 真的变了才回调——因为新增一个「窗口状态」事件名要动协议与
 * 两侧守卫，那是窗口模型层级的决定。
 *
 * 窗口 API 来自 Tauri 注入的全局（`withGlobalTauri: true` 下的 `window.__TAURI__.window`）；
 * 缺席时按本层纪律**留痕降级**（不假装订阅成功）。订阅后立即读一次，调用方拿它当初值。
 */
function subscribeWindowChromeState(
  invoke: TauriInvoke,
  handler: (state: DesktopWindowChromeState) => void,
): () => void {
  const tauri = (globalThis as { __TAURI__?: { window?: { getCurrentWindow?: () => unknown } } })
    .__TAURI__;
  const current = tauri?.window?.getCurrentWindow
    ? (tauri.window.getCurrentWindow() as {
        onResized?: (callback: () => void) => Promise<() => void>;
      })
    : null;
  if (!current || typeof current.onResized !== "function") {
    logger.warn("[tauri-platform] 窗口 API 不可用：onDesktopWindowChromeStateChanged 无法订阅");
    return () => {};
  }

  let disposed = false;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let unlisten: (() => void) | null = null;
  // 只记上一个布尔值（不是上一份状态对象）：去重比的是「状态有没有变」，
  // 留引用会让「外部把那个对象改了」也能骗过比较。
  let lastMaximized: boolean | null = null;

  const refresh = (): void => {
    if (disposed) return;
    void (invoke(GET_WINDOW_CHROME_STATE) as Promise<DesktopWindowChromeState>).then(
      (state) => {
        if (disposed) return;
        if (!state || typeof state.isMaximized !== "boolean") return;
        if (lastMaximized === state.isMaximized) return;
        lastMaximized = state.isMaximized;
        handler(state);
      },
      (error: unknown) => {
        logger.warn("[tauri-platform] 读取窗口 chrome 状态失败", error);
      },
    );
  };
  const schedule = (): void => {
    if (disposed || timer !== null) return;
    timer = setTimeout(() => {
      timer = null;
      refresh();
    }, 120);
  };

  void current.onResized(schedule).then(
    (stop) => {
      if (disposed) {
        stop();
        return;
      }
      unlisten = stop;
    },
    (error: unknown) => {
      logger.warn("[tauri-platform] 订阅窗口 resize 失败", error);
    },
  );
  refresh();

  return () => {
    disposed = true;
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
    unlisten?.();
    unlisten = null;
  };
}

/**
 * 订阅深链事件里某一类 `kind`。
 *
 * `project` 把事件载荷投影成**该订阅方法要的东西**（oauth/payment 是 `url`，share 是
 * `{ shareCode }`，workspace-open 是 `path`）：五种 `kind` 共用同一个事件名，投影是唯一的差别，
 * 所以它必须在这里显式给出来，而不是让每条订阅各自读字段。
 *
 * 返回同步的 `dispose`（接口契约要求退订函数是同步的）与异步的 `ready`
 * （`listen` 本身是异步的，握手前要等它）。
 *
 * 退订与「订阅还没装好就退订」的竞态：`dispose` 先置位，`listen` 兑现时若发现已退订
 * 就立刻自己退订，不留悬挂监听。
 */
function subscribeDeepLink<T>(
  listen: TauriListen,
  kind: DeepLinkEventKind,
  project: DeepLinkProjection<T>,
  callback: (value: T) => void,
): { dispose: () => void; ready: Promise<void> } {
  let disposed = false;
  let unlisten: TauriUnlisten | null = null;

  const ready = listen<DeepLinkEvent>(DEEP_LINK_EVENT, (envelope) => {
    const payload = envelope?.payload;
    // 载荷来自 IPC，不能假设它符合类型：形状不对时**跳过而不是抛**，
    // 一个畸形事件不该打断整条订阅。
    if (!payload || typeof payload !== "object") return;
    if (payload.kind !== kind) return;
    const projected = project(payload);
    if (projected === undefined) return;
    callback(projected);
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
