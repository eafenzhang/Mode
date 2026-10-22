/* eslint-disable max-lines -- 注入脚本集中维护平台面分类（命令、拒绝桩、事件订阅），拆分会破坏与注入测试的逐项对拍。 */
/**
 * `window.mode` 的注入内容（**产品代码**，由 `crate::bridge` 用 `include_str!` 当文本注入到
 * `initialization_script` 的最前面，因此这里必须是纯 JS，不能出现 TS 类型）。
 *
 * ## 为什么注入在这里，而不是 preload
 *
 * Electron 线由 **preload** 提供 `window.mode`；Tauri 没有 preload / `contextBridge` 等价物，
 * 唯一能在**页面脚本之前**执行的手段就是 `WebviewWindowBuilder::initialization_script`
 * （DR-T3-1）。renderer 在**入口模块求值期**就读 `window.mode`
 * （`renderer/src/main.tsx` 顶层的 `createDesktopPlatform(...)`，其对象字面量里有若干非惰性的
 * `window.mode.X ? … : undefined`），所以「注入早于 renderer 第一次访问」是**正确性的一部分**：
 * 缺席时入口模块当场抛、React 永远挂不上。顺序由「本文件拼在桥脚本之前」+「两者都是
 * document-start 注入」保证，并被 `apps/desktop-tauri/test/windowModeInjection.test.mts` 断言。
 *
 * ## 内容口径（沿用 M1 适配层的纪律）
 *
 * M1 的适配层（`packages/ui/src/platform/tauri-invoke-shim.ts`）已经定了这条纪律：
 * **实现的方法真调用；未实现的必选方法显式抛 `not_implemented_until_m3`（不静默返回假值）；
 * 可选方法保持缺席**（调用方本来就用 `?.` 守卫，塞个桩会把「平台不支持就跳过」变成「直接崩」）。
 * 本文件把同一条纪律落到 `window.mode` 这层接口面上，并按**声明的返回形态**把
 * 「必选、M1 未实现」的成员分成两类（分类由 `windowModeInjection.test.mts` 按
 * `packages/client/src/globals.d.ts` 的声明机械对拍）：
 *
 * - `M1_COMMANDS`：M1 切片表的命令，逐个经 Tauri `invoke` 打到壳的 `#[tauri::command]`；
 * - `EVENT_SUBSCRIBED`：走宿主深链事件通道的方法（五种 `kind` 全部有订阅点），按载荷 `kind` 分流；
 * - `REJECTING_REQUIRED`：**调用方会消费返回值**（`Promise<T>`，T ≠ void）的成员——
 *   调用返回一个带 `not_implemented_until_m3: <name>` 的**被拒 Promise**（不返回假值、
 *   也不同步抛：同步抛会掀翻正在渲染的树，实测见下）；
 * - `NO_OP_REQUIRED`：**没有要读的值**（返回 disposer / `void` / `Promise<void>`）的成员——
 *   降级为空操作并**留痕**。这条不是口味问题，是实测逼出来的：渲染层挂载时就会调
 *   `window.mode.onRemoteConnectionLog(handler)` 且**不加守卫**，一个抛错的桩会当场掀翻整棵树
 *   （错误边界接住，界面变错误屏）；而「这个宿主不发这类事件」的正确语义就是空操作。
 *   原始输出见 `docs/specs/tauri-renderer-host.md` §10.8；
 * - `DELIBERATELY_ABSENT_REQUIRED`：唯一例外，见下面的注释。
 *
 * 成员清单不是手抄的「感觉」，而且**两处真源都要对**：
 *
 * - `packages/client/src/globals.d.ts` 是 `window.mode` 的声明真源——按声明返回形态自动分类；
 * - **renderer 的调用面**是第二处真源：它引用了 123 个成员、其中 11 个**没有**在那份声明里
 *   （它们由 `IPlatformService`（`packages/shared/src/platform.ts`）声明，适配层直接透传）。
 *   本文件据此补上渲染层**不加守卫就会调**的那些（`loadMcpFromUserDirectory`、
 *   `saveMcpToUserDirectory`、`migrateLegacyCommonMcp`、`renameLanPairedPeer`；
 *   `createTempTextAttachment` 已从拒绝桩升级为真命令，见 M1_COMMANDS），
 *   分类同样按 `IPlatformService` 的返回形态。
 *
 * `windowModeInjection.test.mts` 两头都守：声明面逐名对拍 + **扫渲染层源码取出「被直接调用
 * （没有 `?.`）的成员名」再要求它们全部被覆盖**。只守声明面是不够的——「从一个文件枚举面、
 * 而消费者在另一个文件调用」正是本仓库反复吃亏的那类缺口（review I1）。
 *
 * ## 依赖 `withGlobalTauri`
 *
 * **命令通道**走 `window.__TAURI__.core.invoke`，兜底 `window.__TAURI_INTERNALS__.invoke`
 * （见 `commandChannel` 的注释：前者依赖 `withGlobalTauri` 开关，后者是 Tauri 注入 IPC 的必到那一路）。
 * **事件订阅**只能走 `window.__TAURI__.event.listen`（`tauri.conf.json` 的 `app.withGlobalTauri: true`），
 * 缺席时降级为 no-op 订阅并**留痕**（`no_event_api`）。
 * **两者都延迟解析**：Tauri 注入全局与用户 init script 的先后顺序不是本层能约定的，
 * 所以在脚本求值期**不**读它们，只在方法被调用时才取——否则一个顺序差异就会让
 * `window.mode` 整个缺席。
 *
 * ## 已知缺口（登记，不静默）
 *
 * - `log`：`window.mode.log` 在类型里是必选，但 renderer 的调用点是 `mode?.log?.(…)`（可选链），
 *   而「日志」这条能力的真形态需要壳新增一条命令（不在本任务授权内）。这里**故意留空**：
 *   塞一个抛错的桩会把「日志转发不可用」升级成「logger 自己抛」，比缺口更糟。
 *   缺口可观察：`window.__MODE_WINDOW_MODE__.deliberatelyAbsent`。
 * - 事件 API 缺席时（`withGlobalTauri` 被关掉）订阅降级为 no-op disposer 并**留痕**
 *   （`degraded` 计数 + console.warn），不是静默成功。
 *
 * ## M3 Task 4 的追加（深链面到此完整）
 *
 * `EVENT_SUBSCRIBED` 从 2 条扩到 4 条：`onShareImport` / `onOpenWorkspacePath` 在接口声明里就是
 * **深链驱动**的，而宿主从 M1 起就投递 `share-import` / `workspace-open` 两个 `kind`——只是没有
 * 订阅点（症状：链接到了渲染层却没人接）。**五种 `kind` 现在都有落点**（`unrecognized` 例外，
 * 它本来就是「收到了但没人认领」的观察面）。其余订阅型方法的逐条登记见
 * `docs/specs/tauri-platform-commands.md` §9.4。
 */
(function () {
  var NOT_IMPLEMENTED_PREFIX = "not_implemented_until_m3";
  var DEEP_LINK_EVENT = "mode:deep-link";
  var UPDATE_STATE_EVENT = "mode:update-state";
  var KIND_OAUTH = "oauth-callback";
  var KIND_PAYMENT = "payment-callback";
  // M3 Task 4 补的两条：`share-import` / `workspace-open` 从 M1 起就由宿主的路由投递
  // （`crates/deeplink`），只是一直没有订阅点——于是链接到了渲染层却没人接。
  var KIND_SHARE_IMPORT = "share-import";
  var KIND_WORKSPACE_OPEN = "workspace-open";
  // 命令名由 Rust 侧注入（`crate::bridge` 的占位符替换），不手抄：wire 名的唯一真源在那边。
  var CMD_PLATFORM_REPORT = "__MODE_HOST_CMD_PLATFORM_REPORT__";
  // 窗口控制面的两个命令（壳 crate 的 `window_controls.rs`；`bridge.rs` 持有名字的真源）。
  var CMD_EXECUTE_DESKTOP_COMMAND = "__MODE_HOST_CMD_EXECUTE_DESKTOP_COMMAND__";
  var CMD_WINDOW_CHROME_STATE = "__MODE_HOST_CMD_WINDOW_CHROME_STATE__";
  /**
   * 本地「打开方式」胶囊的最小可用编辑器面：只声明系统文件管理器。
   *
   * 根因（2026-10-20 真机验收）：`WorkspaceEditorButtonGroup` 依赖 `getInstalledEditors()`；
   * Tauri 注入层此前把它放在 REJECTING_REQUIRED 里，调用被拒后 `installedEditors` 为空，
   * `selectedEditor` 为 null，整颗胶囊不渲染，本地工作区右上角就没有资源管理器入口。
   * Electron 线由 main 的 editors.ts 返回完整列表；Tauri 壳这轮只补「系统文件管理器」
   * 这一条真实可用的面，不伪造未安装的编辑器。
   *
   * 图标刻意留空：Tauri 拿不到系统图标位图，UI 会回退到既有 `FolderOpenIcon`
   * （与远端 `WorkspaceFileManagerButton` 的兜底同一个图标），不再注入自造 SVG。
   */
  function fileManagerEditorInfo() {
    var ua =
      typeof navigator !== "undefined" && typeof navigator.userAgent === "string"
        ? navigator.userAgent
        : "";
    if (/Windows/i.test(ua)) {
      return { id: "explorer", name: "资源管理器", iconDataUrl: "" };
    }
    if (/Mac OS X|Macintosh/i.test(ua)) {
      return { id: "finder", name: "Finder", iconDataUrl: "" };
    }
    return { id: "xdg-open", name: "文件管理器", iconDataUrl: "" };
  }

  /** M1 切片表 + LR-3/LR-5 追加的命令（与 `crates/platform/src/commands/mod.rs` 的切片表同名）。 */
  var M1_COMMANDS = [
    "openExternal",
    "selectDirectory",
    "selectFile",
    "saveFile",
    // LR-5：长粘贴文本 → 临时附件（Electron main IPC 已随旧线 EOL 删除）。
    "createTempTextAttachment",
    "openInFileManager",
    // 用系统默认应用打开本地文件（目录仍走 openInFileManager）。
    "openExternalFile",
    "showTaskNotification",
    "notifyRendererReady",
    "syncWindowTabs",
    "syncWindowUnreadCount",
    "getApplicationIcon",
    // LR-3：远程桌面剪贴板读取（壳能力，`crates/platform/src/commands/clipboard.rs`）。
    "rdReadClipboardText",
  ];
  /**
   * 通过**宿主深链事件通道**实现的订阅方法（不在命令表里）。
   *
   * M1 落前两条；M3 Task 4 把后两条也接上——它们在接口声明里就是**深链驱动**的
   * （`mode://share/import?code=…` 与「deep link 直接打开本地工作区目录」），
   * 而宿主从 M1 起就投递这两个 `kind`。至此宿主认得的五种深链在本层**全部有落点**。
   */
  var EVENT_SUBSCRIBED = [
    "onOAuthCallback",
    "onPaymentCallback",
    "onShareImport",
    "onOpenWorkspacePath",
  ];
  /**
   * 窗口控制面（M3 Task 4 fix round）：产品 UI 的**自绘标题栏**就调这几个。
   *
   * 从前它们分别是「降级空操作」（`executeDesktopCommand`，必选、返回 `Promise<void>`）与
   * 「缺席」（`getDesktopWindowChromeState` / `onDesktopWindowChromeStateChanged`，可选）——
   * 症状是**用户点了最小化/最大化/关闭没有任何反应**，而系统标题栏还画在同一条位置上
   * （两条标题栏）。现在三个都走壳的窗口控制面（`window_controls.rs`），系统标题栏在产品窗口上
   * 也去掉了（`mode_tauri::window` 的 `decorations`）。
   */
  var WINDOW_CONTROL = [
    "executeDesktopCommand",
    "getDesktopWindowChromeState",
    "onDesktopWindowChromeStateChanged",
  ];
  /**
   * 自动更新面（壳的 updater `extra_handler` + `mode:update-state` 事件）。
   * 不在 M1 命令切片里，但注入层是实现；诊断与测试都按这份清单认「已实现」。
   *
   * `onUpdateReady` 也在这份清单里：它不是一条独立事件，而是**从 `mode:update-state` 派生**的
   * 兼容视图——壳没有第二个「ready」事件源，`kind === "update-downloaded"` 就是「已下载完毕」。
   * 派生规则与 Electron 线 preload 的缓存语义一致，见下面 `applyUpdateState` 的注释。
   */
  var UPDATE_STATE = [
    "getUpdateState",
    "onUpdateStateChanged",
    "onUpdateReady",
    "downloadUpdate",
    "cancelUpdateDownload",
    "quitAndInstallUpdate",
    "skipUpdateVersion",
    "getAutoUpdatePreferences",
    "setAutoDownloadAndInstallUpdates",
  ];
  /**
   * 必选、M1 未实现，且**调用方会消费返回值**的成员：调用返回一个**被拒的 Promise**。
   *
   * 判据是机械的（`windowModeInjection.test.mts` 按声明返回值对拍）：声明返回
   * `Promise<T>`（T ≠ void）—— 也就是「调用方会读这个值」。给它们返回假值会把「能力不存在」
   * 伪装成事实（`isDockerAvailable()` 返回 `false` 会被读成「本机没有 Docker」），
   * 所以**绝不返回假值**。
   *
   * **为什么是「被拒的 Promise」而不是同步抛**（M3 Task 3 的真机跑逼出来的，两轮）：
   * 渲染层在**渲染期**就会不加守卫地调这类方法（实测 `canOpenCommunity`），同步抛出会变成
   * 一次 render 错误——错误边界把整个界面区域换成错误屏（原始输出见 spec §10.8/§10.9）。
   * 被拒的 Promise 保留了 M1 口径的全部价值（错误里带着方法名，日志/诊断能看到，
   * 不是 `undefined is not a function`、也不是假值），但把失败**限制在调用点的 Promise 语义里**，
   * 不会掀翻一棵正在渲染的树。**注入层不再同步抛任何东西。**
   */
  var REJECTING_REQUIRED = [
    "connectRemote",
    "isDockerAvailable",
    "listWSLDistros",
    "listDockerContainers",
    "listSSHConfigAliases",
    "getLanAccessState",
    "setLanAccessEnabled",
    "createLanAccessPairCode",
    "removeLanAccessClient",
    "resetLanAccessTokens",
    "discoverLanPeers",
    "pairLanPeer",
    "getLanPairedPeers",
    "getLanPeerWorkspaces",
    "canOpenCommunity",
    "loadMcpFromUserDirectory",
    "migrateLegacyCommonMcp",
    "exportLogs",
    "rdGetPeerCapability",
    "rdStartSession",
    "rdGetControlledState",
    "rdGetServerState",
    "rdSetServerEnabled",
    "rdRevokeClient",
    "rdAuthorizeClient",
    "saveMcpToUserDirectory",
  ];
  /**
   * 必选、M1 未实现，但**调用方不消费返回值**的成员：留一个「降级空操作」并留痕。
   *
   * 这一类是**实测逼出来的**（M3 Task 3 的真机跑）：渲染层挂载时就会调
   * `window.mode.onRemoteConnectionLog(handler)`（未加 `?.` 守卫），一个抛错的桩会当场
   * 掀翻整棵树（错误边界接住了，界面变成错误屏——原始输出见 spec §10.8）。
   * 而这类成员的声明形态正是「没有要读的值」：返回 disposer（订阅）、返回 `void`
   * （通知型命令）、返回 `Promise<void>`（异步通知），所以**空操作**才是它该有的语义
   * ——「这个宿主不发这类事件 / 不实现这条通知」，而不是「调用即崩」。
   *
   * **不静默**：每次调用记进 `window.__MODE_WINDOW_MODE__.degraded`（按方法名去重）并
   * `console.warn` 一次，所以缺口是可观察的，不是「看着像成功」。
   *
   * 返回的是一个**可调用且可 await** 的空操作（见 `makeNoOp`）：三种消费形态
   * （`const dispose = f(h)` / `void f(x)` / `await f(x)`）都能成立，调用方不会因为
   * 「返回了 undefined 又拿去调用」而二次崩。
   */
  var NO_OP_REQUIRED = [
    "disposeRemoteSession",
    "removeLanPairedPeer",
    "renameLanPairedPeer",
    "onRemoteConnectionLog",
    "onRemoteSessionClosed",
    "onBotRemoteWorkspaceReconnected",
    "syncActiveTaskSession",
    "onFocusTab",
    "onNewTab",
    "onNewTask",
    "onWindowFullscreenChanged",
    "onTaskNotificationClick",
    "registerOAuthState",
    "syncTelemetryContext",
    "reportTelemetryEvent",
    "onUpdateCheckResult",
    "onPostUpdateReleaseNotes",
    "acknowledgePostUpdateReleaseNotes",
    "setApplicationLocale",
    "setTitleBarTheme",
    "rdSendSignal",
    "rdEndSession",
    "onRemoteDesktopSessionEvent",
  ];
  /** 必选但**故意留空**的成员（理由见文件头「已知缺口」）。 */
  var DELIBERATELY_ABSENT_REQUIRED = ["log"];

  // 诊断上报的次数上限（见文件末的 reportPlatformSurface）：在**任何**降级可能发生之前就初始化，
  // 免得「计数还是 undefined」把上限变成 NaN 从而失效。
  var MAX_SURFACE_REPORTS = 5;
  var surfaceReports = 0;

  var diagnostics = {
    source: "sidecar-host/window_mode.ts",
    implemented: M1_COMMANDS.slice()
      .concat(EVENT_SUBSCRIBED)
      .concat(WINDOW_CONTROL)
      .concat(UPDATE_STATE),
    notImplemented: REJECTING_REQUIRED.slice(),
    noOp: NO_OP_REQUIRED.slice(),
    deliberatelyAbsent: DELIBERATELY_ABSENT_REQUIRED.slice(),
    degraded: [],
  };
  window.__MODE_WINDOW_MODE__ = diagnostics;

  var degradedSeen = {};
  function degrade(reason, detail) {
    // 去重：被降级的成员可能每帧都被调用，同一个理由只记（并只打印）一次。
    if (degradedSeen[reason]) return;
    degradedSeen[reason] = true;
    diagnostics.degraded.push(reason);
    // 降级是**事后**发生的（渲染层挂载时才调那些方法），所以每出现一个新理由就更新一条记录。
    reportPlatformSurface();
    try {
      console.warn("[mode-window] " + reason + ": " + String(detail));
    } catch {
      /* 诊断本身失败就放弃 */
    }
  }

  /**
   * 降级空操作：**可调用**（订阅型调用方会把它当 disposer 调用）**且可 await**
   * （异步通知型调用方会 `await` / `.catch`），同时忽略返回值的调用方也不受影响。
   *
   * 三种形态共用一个值，是因为注入层拿不到调用点的消费方式，而声明里这三类都存在；
   * 一个「既非函数也非 thenable」的返回（例如裸 `undefined`）会让订阅型调用方
   * 在下一行二次崩（`dispose is not a function`），那比缺口本身更糟。
   */
  function makeNoOp(method) {
    function noop() {}
    // thenable 是**刻意设计**的一部分：「没有要读的值」的成员在声明里既有返回 disposer 的、
    // 也有返回 `Promise<void>` 的，而注入层拿不到调用点的消费方式；一个既非函数也非 thenable
    // 的返回（例如裸 `undefined`）会让订阅型调用方在下一行二次崩（`dispose is not a function`），
    // 比缺口本身更糟。这条与「可调用」一起被 `windowModeInjection.test.mts` 的三种消费形态断言钉住。
    // oxlint 的 `unicorn/no-thenable` 是这条设计的**预期**告警，这里显式关掉它（本仓只加了这一条）。
    // eslint-disable-next-line unicorn/no-thenable
    noop.then = function (onFulfilled) {
      if (typeof onFulfilled === "function") onFulfilled(undefined);
      return noop;
    };
    noop.catch = function () {
      return noop;
    };
    noop.finally = function (onFinally) {
      if (typeof onFinally === "function") onFinally();
      return noop;
    };
    return function () {
      degrade("not_implemented_noop:" + method, method);
      return noop;
    };
  }

  /**
   * 延迟解析命令通道：注入那一刻它可能还没到（见文件头），所以只在方法被调用时才取。
   *
   * **两条候选，顺序固定**：`window.__TAURI__.core.invoke`（`withGlobalTauri` 暴露的公开面）
   * 优先，`window.__TAURI_INTERNALS__.invoke` 兜底（Tauri 自己的 IPC 入口，桥脚本已经在用它
   * ——真机实测证明它在 document-start 就在）。为什么不只用前者：`withGlobalTauri` 是
   * `tauri.conf.json` 的一个开关，关掉它不该让整层平台面（以及诊断上报）一起消失；
   * 而 `__TAURI_INTERNALS__` 是 Tauri 注入 IPC 的**必到**那一路。
   */
  function commandChannel() {
    var api = window.__TAURI__;
    if (api && api.core && typeof api.core.invoke === "function") return api.core;
    var internals = window.__TAURI_INTERNALS__;
    if (internals && typeof internals.invoke === "function") return internals;
    return null;
  }

  function invoke(command, args) {
    var channel = commandChannel();
    // 不静默吞：两条通道都没有 = 壳没配好，必须是响亮的失败，而不是「命令返回空」。
    if (!channel) throw new Error("mode_window_no_tauri_api: " + command);
    return channel.invoke(command, args);
  }

  /** 返回 `void` 的命令只能发后不管；失败必须留痕（IPC 断/命令改名都在这里现形）。 */
  function fireAndForget(command, args) {
    try {
      invoke(command, args).catch(function (error) {
        degrade("invoke_failed:" + command, error);
      });
    } catch (error) {
      degrade("invoke_threw:" + command, error);
    }
  }

  /**
   * 裸字符串形态的 `getApplicationIcon` 入参归一化。
   *
   * 与 `@mode/ui/tauri-platform` 的 `normalizeApplicationIconRequest` 同一口径（Rust 命令只收
   * 结构化 locator；字符串在该接口的历史语义里随平台而异，所以三种解释都交出去逐个尝试）。
   * 两处的一致性由 `windowModeInjection.test.mts` 对拍（把同一个字符串喂给两边，比 `invoke` 的入参）。
   */
  function normalizeIconRequest(request) {
    if (typeof request !== "string") return request;
    return {
      locators: [
        { kind: "windows-aumid", value: request },
        { kind: "windows-executable-path", value: request },
        { kind: "darwin-bundle-id", value: request },
      ],
    };
  }

  /** `url` 投影（oauth / payment）：形状不对给 `undefined`，由 subscribeDeepLink 跳过。 */
  function projectUrl(payload) {
    return typeof payload.url === "string" && payload.url.length > 0 ? payload.url : undefined;
  }

  /** `path` 投影（`workspace-open`）。 */
  function projectPath(payload) {
    return typeof payload.path === "string" && payload.path.length > 0 ? payload.path : undefined;
  }

  /** `{ shareCode }` 投影（`share-import`）：与 `IPlatformService.onShareImport?` 的载荷同形。 */
  function projectShareCode(payload) {
    return typeof payload.code === "string" && payload.code.length > 0
      ? { shareCode: payload.code }
      : undefined;
  }

  /**
   * 命令通道的**可等待**包装：同步抛出也转成被拒的 Promise。
   *
   * 为什么需要：窗口控制与 chrome 状态的调用点是 `void platform.executeDesktopCommand(id).catch(…)`
   * （`DesktopWindowControls.tsx`）——`invoke()` 在「两条通道都没有」时是**同步抛**，
   * 那会直接穿出调用方的 `.catch`，在渲染期变成一次 render 错误（本层反复强调的形态）。
   * 所以这里统一转成 Promise：失败留在调用点的 Promise 语义里，且**留痕**。
   */
  function invokeAsync(command, args) {
    try {
      return invoke(command, args);
    } catch (error) {
      degrade("invoke_threw:" + command, error);
      return Promise.reject(error);
    }
  }

  /**
   * 读一次窗口 chrome 状态（`DesktopWindowChromeState`：`{ isMaximized, … }`）。
   *
   * 形状校验收在**一个**地方：`isMaximized` 不是布尔就当读取失败（`Promise.reject` + 留痕），
   * 不把 `undefined` 交给渲染层去画图标——那会变成「图标随机」而不是「读取失败」。
   */
  function readWindowChromeState() {
    return invokeAsync(CMD_WINDOW_CHROME_STATE).then(function (state) {
      if (!state || typeof state !== "object" || typeof state.isMaximized !== "boolean") {
        degrade("chrome_state_shape", CMD_WINDOW_CHROME_STATE);
        return Promise.reject(new Error("mode_window_chrome_state_invalid"));
      }
      return state;
    });
  }

  /**
   * 订阅窗口 chrome 状态变化，返回同步的 disposer。
   *
   * **为什么不是一条新事件通道**：最大化/还原/全屏都会让窗口尺寸变化，所以这里用 Tauri **内置**的
   * 窗口 `onResized` 事件做触发点，去抖后**重读**一次状态，只在 `isMaximized` 真的变了才回调。
   * 新增一个「窗口状态」事件名要动协议、生成物与两侧守卫（那是窗口模型层级的决定，本任务没有授权），
   * 而这条路用的全是既有能力。代价写在 spec 里：一次状态变化最多带来一次多余的读取（去抖 120ms）、
   * 以及「尺寸没变的状态变化不会被观察到」——在最大化/还原/全屏这三种情形下尺寸一定变。
   */
  function subscribeWindowChromeState(handler) {
    var api = window.__TAURI__;
    var current =
      api && api.window && typeof api.window.getCurrentWindow === "function"
        ? api.window.getCurrentWindow()
        : null;
    if (!current || typeof current.onResized !== "function") {
      // 降级但**不静默**（与深链订阅同款）：订阅装不上不是「订阅成功但没有回调」。
      degrade("no_window_api", "onDesktopWindowChromeStateChanged");
      return function () {};
    }
    var disposed = false;
    var timer = null;
    var stop = null;
    // 只记**上一个布尔值**（不是上一份状态对象）：IPC 每次给的是新对象，但把引用留着等于
    // 让「外部对象被改动」也能骗过比较——那正是这条去重的反面。
    var lastMaximized = null;
    function refresh() {
      if (disposed) return;
      readWindowChromeState().then(
        function (state) {
          if (disposed) return;
          if (lastMaximized === state.isMaximized) return;
          lastMaximized = state.isMaximized;
          handler(state);
        },
        function (error) {
          degrade("chrome_state_failed", error);
        },
      );
    }
    // 去抖：拖拽改变尺寸会连发事件，而这里只关心「状态是不是变了」。
    function schedule() {
      if (disposed || timer !== null) return;
      timer = setTimeout(function () {
        timer = null;
        refresh();
      }, 120);
    }
    void current.onResized(schedule).then(
      function (unlisten) {
        if (disposed) {
          unlisten();
          return;
        }
        stop = unlisten;
      },
      function (error) {
        degrade("subscribe_failed:onDesktopWindowChromeStateChanged", error);
      },
    );
    // 订上就先读一次：调用方（标题栏）拿它当订阅后的初值。
    refresh();
    return function () {
      disposed = true;
      if (timer !== null) {
        clearTimeout(timer);
        timer = null;
      }
      if (typeof stop === "function") stop();
      stop = null;
    };
  }

  /**
   * 订阅更新状态事件（`onUpdateStateChanged`），返回同步的 disposer。
   *
   * 事件名 `mode:update-state` 的真源是 `crates/updater/src/state.rs` 的 `UPDATE_STATE_EVENT`；
   * 写错是**静默失效**（订阅装上了却永远收不到），所以由验收用例把两边对拍。
   */
  function subscribeUpdateState(callback) {
    var api = window.__TAURI__;
    if (!api || !api.event || typeof api.event.listen !== "function") {
      degrade("no_event_api", "update-state");
      return function () {};
    }
    var disposed = false;
    var stop = null;
    void api.event
      .listen(UPDATE_STATE_EVENT, function (envelope) {
        var payload = envelope && envelope.payload;
        if (payload && typeof payload === "object") {
          callback(payload);
        }
      })
      .then(
        function (unlisten) {
          if (disposed) {
            unlisten();
            return;
          }
          stop = unlisten;
        },
        function (error) {
          degrade("subscribe_failed:update-state", error);
        },
      );
    return function () {
      disposed = true;
      if (typeof stop === "function") stop();
      stop = null;
    };
  }

  /**
   * `onUpdateReady` 的共享缓存与派生（见 `UPDATE_STATE` 里的说明）。
   *
   * Electron 线的语义分两条事件：`update-ready` 触发回调，`update-state-changed` 维护缓存
   * （`kind === "update-downloaded"` 时记版本，其它状态清空）。Tauri 线只有后一条，所以在这里
   * 用**同一份状态机**推导出前者：
   *
   * - 进入 `update-downloaded` 且版本变了 → 通知订阅方；
   * - 离开 `update-downloaded`（取消、跳过、staging error）→ 清空缓存，避免 UI 拿失效版本
   *   撑出「重启以更新」按钮；
   * - 订阅时回放缓存：`update-downloaded` 只出现一次，晚挂载的组件否则永远收不到。
   *
   * 首次订阅还会用 `getUpdateState` 补一次快照（页面加载可能晚于下载完成，此时没有新事件）。
   * 快照与实时事件用 `updateStateRevision` 去重：快照在途期间来了实时事件，就以实时事件为准。
   */
  var updateReadyCache = null;
  var updateReadyCallbacks = [];
  var updateStateRevision = 0;
  var updateStateBridgeStarted = false;

  function projectReadyVersion(payload) {
    if (!payload || typeof payload !== "object") return null;
    if (payload.kind !== "update-downloaded") return null;
    return typeof payload.version === "string" && payload.version.length > 0
      ? payload.version
      : null;
  }

  function notifyUpdateReady(version) {
    var callbacks = updateReadyCallbacks.slice();
    for (var index = 0; index < callbacks.length; index += 1) {
      try {
        callbacks[index](version);
      } catch (error) {
        // 一个订阅方抛错不该打断其它订阅方（与深链订阅同一条纪律）。
        degrade("update_ready_callback_failed", error);
      }
    }
  }

  function applyUpdateState(payload) {
    updateStateRevision += 1;
    var next = projectReadyVersion(payload);
    var previous = updateReadyCache;
    updateReadyCache = next;
    if (next === null || next === previous) return;
    notifyUpdateReady(next);
  }

  function ensureUpdateStateBridge() {
    if (updateStateBridgeStarted) return;
    updateStateBridgeStarted = true;
    // 订阅先装、快照后取：`subscribeUpdateState` 同步返回，listen 的 Promise 在内部兑现。
    // 这样「快照在途时来了实时事件」一定被 revision 拦住，不会用旧快照覆盖新状态。
    subscribeUpdateState(applyUpdateState);
    var seededRevision = updateStateRevision;
    try {
      Promise.resolve(invoke("getUpdateState")).then(
        function (snapshot) {
          if (updateStateRevision !== seededRevision) return;
          applyUpdateState(snapshot);
        },
        function (error) {
          degrade("update_state_seed_failed", error);
        },
      );
    } catch (error) {
      degrade("update_state_seed_threw", error);
    }
  }

  function subscribeDeepLink(kind, project, callback) {
    var api = window.__TAURI__;
    if (!api || !api.event || typeof api.event.listen !== "function") {
      // 降级但**不静默**：缺事件 API 是「订阅装不上」，不是「订阅成功但没有回调」。
      degrade("no_event_api", kind);
      return function () {};
    }
    var disposed = false;
    var stop = null;
    void api.event
      .listen(DEEP_LINK_EVENT, function (envelope) {
        var payload = envelope && envelope.payload;
        // 载荷来自 IPC，不能假设它符合类型：形状不对就跳过，一个畸形事件不该打断整条订阅。
        if (!payload || typeof payload !== "object") return;
        if (payload.kind !== kind) return;
        var projected = project(payload);
        if (projected === undefined) return;
        callback(projected);
      })
      .then(
        function (unlisten) {
          if (disposed) {
            unlisten();
            return;
          }
          stop = unlisten;
        },
        function (error) {
          degrade("subscribe_failed:" + kind, error);
        },
      );
    return function () {
      disposed = true;
      if (typeof stop === "function") stop();
      stop = null;
    };
  }

  var mode = {
    // MCP 目录读写仍由 REJECTING_REQUIRED 拒绝；渲染层据此改用 host 的 mcpSyncService。
    supportsLocalMcpDirectory: false,
    // 资源管理器窗口尚未移植到 Tauri 壳；帮助菜单据此隐藏该入口。
    supportsResourceManager: false,
    openExternal: function (url) {
      fireAndForget("openExternal", { url: url });
    },
    selectDirectory: function () {
      return invoke("selectDirectory");
    },
    selectFile: function () {
      return invoke("selectFile");
    },
    saveFile: function (payload) {
      return invoke("saveFile", { payload: payload });
    },
    createTempTextAttachment: function (payload) {
      return invoke("createTempTextAttachment", { payload: payload });
    },
    openInFileManager: function (path) {
      return invoke("openInFileManager", { path: path });
    },
    openExternalFile: function (path) {
      return invoke("openExternalFile", { path: path });
    },
    // 本地「打开方式」胶囊的数据源（见文件头的 fileManagerEditorInfo 注释）。
    // 只返回系统文件管理器这一条真实可用的面；远端工作区由 WorkspaceFileManagerButton 承担。
    getInstalledEditors: function () {
      return Promise.resolve([fileManagerEditorInfo()]);
    },
    // 文件管理器 id 委托既有壳命令；未知 id / 远端目标失败关闭，不静默成功。
    openInEditor: function (editorId, path, options) {
      var fileManager = fileManagerEditorInfo();
      if (editorId !== fileManager.id) {
        return Promise.resolve({
          success: false,
          error: "editor_unavailable: " + String(editorId),
        });
      }
      if (options && options.remoteTarget) {
        return Promise.resolve({
          success: false,
          error: "remote_editor_unsupported",
        });
      }
      return invoke("openInFileManager", { path: path }).then(
        function (result) {
          // 壳命令的返回形态是 { success, error? }；缺字段时按成功处理，
          // 与 Electron main 的 openPathViaShell 语义保持一致。
          if (result && typeof result === "object" && typeof result.success === "boolean") {
            return result;
          }
          return { success: true };
        },
        function (error) {
          return {
            success: false,
            error: String(error && error.message ? error.message : error),
          };
        },
      );
    },
    showTaskNotification: function (payload) {
      fireAndForget("showTaskNotification", { payload: payload });
    },
    notifyRendererReady: function () {
      fireAndForget("notifyRendererReady");
    },
    syncWindowTabs: function (paths) {
      fireAndForget("syncWindowTabs", { paths: paths });
    },
    syncWindowUnreadCount: function (count) {
      fireAndForget("syncWindowUnreadCount", { count: count });
    },
    // 类型里是可选成员（renderer 用 `?.` 守卫），但 M1 命令面把它实现了，所以给出真实现。
    getApplicationIcon: function (request) {
      return invoke("getApplicationIcon", { request: normalizeIconRequest(request) });
    },
    // LR-3：远程桌面主控剪贴板同步的读取口（壳命令 `rd_read_clipboard_text`）。
    rdReadClipboardText: function () {
      return invoke("rdReadClipboardText");
    },
    // 窗口控制面：命令 id 的取值集合来自 `DesktopCommandIds`（`@mode/shared`），
    // 壳侧只实现窗口级的那些（见 `window_controls.rs`），其余 id 带回 `success: false`。
    executeDesktopCommand: function (command) {
      return invokeAsync(CMD_EXECUTE_DESKTOP_COMMAND, { command: command });
    },
    getDesktopWindowChromeState: function () {
      return readWindowChromeState();
    },
    onDesktopWindowChromeStateChanged: function (handler) {
      return subscribeWindowChromeState(handler);
    },
    getUpdateState: function () {
      return invoke("getUpdateState");
    },
    onUpdateStateChanged: function (callback) {
      return subscribeUpdateState(callback);
    },
    // 更新就绪是**派生视图**：订阅即回放缓存，状态离开 update-downloaded 即清除（见上面的注释）。
    onUpdateReady: function (callback) {
      if (typeof callback !== "function") {
        degrade("update_ready_invalid_callback", typeof callback);
        return function () {};
      }
      ensureUpdateStateBridge();
      updateReadyCallbacks.push(callback);
      var cached = updateReadyCache;
      if (cached !== null) {
        try {
          callback(cached);
        } catch (error) {
          degrade("update_ready_callback_failed", error);
        }
      }
      return function () {
        var index = updateReadyCallbacks.indexOf(callback);
        if (index !== -1) updateReadyCallbacks.splice(index, 1);
      };
    },
    downloadUpdate: function () {
      return invoke("downloadUpdate");
    },
    cancelUpdateDownload: function () {
      return invoke("cancelUpdateDownload");
    },
    quitAndInstallUpdate: function () {
      return invoke("quitAndInstallUpdate");
    },
    skipUpdateVersion: function (version) {
      return invoke("skipUpdateVersion", { version: version });
    },
    getAutoUpdatePreferences: function () {
      return invoke("getAutoUpdatePreferences");
    },
    setAutoDownloadAndInstallUpdates: function (enabled) {
      return invoke("setAutoDownloadAndInstallUpdates", { enabled: enabled });
    },
    onOAuthCallback: function (callback) {
      return subscribeDeepLink(KIND_OAUTH, projectUrl, callback);
    },
    onPaymentCallback: function (callback) {
      return subscribeDeepLink(KIND_PAYMENT, projectUrl, callback);
    },
    // 类型里是可选的（调用方用 `?.` 守卫），但宿主**确实会发**这两个 `kind`，
    // 所以给出真实现而不是留空——「平台不支持」与「这条链路没接」是两件事。
    onShareImport: function (callback) {
      return subscribeDeepLink(KIND_SHARE_IMPORT, projectShareCode, callback);
    },
    onOpenWorkspacePath: function (callback) {
      return subscribeDeepLink(KIND_WORKSPACE_OPEN, projectPath, callback);
    },
  };

  for (var rejectingIndex = 0; rejectingIndex < REJECTING_REQUIRED.length; rejectingIndex += 1) {
    mode[REJECTING_REQUIRED[rejectingIndex]] = (function (method) {
      return function () {
        // 留痕（去重）+ 一个带方法名的拒绝：失败可诊断，但不掀翻正在渲染的树。
        degrade("not_implemented_rejected:" + method, method);
        return Promise.reject(new Error(NOT_IMPLEMENTED_PREFIX + ": " + method));
      };
    })(REJECTING_REQUIRED[rejectingIndex]);
  }
  for (var noOpIndex = 0; noOpIndex < NO_OP_REQUIRED.length; noOpIndex += 1) {
    mode[NO_OP_REQUIRED[noOpIndex]] = makeNoOp(NO_OP_REQUIRED[noOpIndex]);
  }

  window.mode = mode;

  /**
   * 把平台面清单送上**产品诊断面**（review I3 的处置）。
   *
   * 为什么需要它：`diagnostics.degraded` 只在页面里，而 `console` 与 devtools 在 release 里
   * 都不存在（`tauri = "2"` 不启 devtools，`console.warn` 也没有捕获方）——于是「没有要读的值的
   * 成员被降级为空操作」这件事在真机上**不可见**，而调用方收到的还是**成功**。这条命令把清单送到
   * 壳的 stderr（与 Host 的结构化失败行同一条通道，壳本来就把它当诊断面），让「能力缺失」在产品的
   * 诊断记录里留下一条。
   *
   * 时序：`DOMContentLoaded` 时先报一次（此时只能报**清单**，降级还没发生）；此后每出现一个新的
   * 降级理由再报一次，**上限 `MAX_SURFACE_REPORTS` 次**（去重后的降级至多几十条，不设上限会把
   * stderr 刷满）。每一次报的都是**当前完整**清单，所以最后一条就是全量。
   */
  // 函数声明会提升，所以 `degrade`（可能在任何位置被调用）可以安全地引用它。
  function reportPlatformSurface() {
    if (!(surfaceReports < MAX_SURFACE_REPORTS)) return;
    surfaceReports += 1;
    // **载荷必须包在参数名里**：Tauri v2 按 `#[tauri::command]` 的参数名映射入参，
    // 壳侧是 `host_platform_report(report: PlatformSurfaceReport)`，所以传 `{ report: … }`。
    // 传错形状时 invoke 会拒绝（症状：清单永远不出现），所以这条由 CI 的 vm 用例钉住
    // （`windowModeInjection.test.mts` 断言 invoke 的入参形状）。
    var payload = {
      report: {
        implemented: diagnostics.implemented,
        notImplemented: diagnostics.notImplemented,
        noOp: diagnostics.noOp,
        deliberatelyAbsent: diagnostics.deliberatelyAbsent,
        degraded: diagnostics.degraded,
      },
    };
    try {
      invoke(CMD_PLATFORM_REPORT, payload).catch(function (error) {
        // 上报失败也要留痕（否则「清单永远不出现」这种症状没有任何线索）：
        // 记进账本 —— 下一次成功的上报会把它带走；`degrade` 按理由去重，所以不会自激。
        degrade("platform_report_failed", error);
      });
    } catch (error) {
      degrade("platform_report_threw", error);
    }
  }

  if (typeof document !== "undefined" && document.addEventListener) {
    if (document.readyState === "loading") {
      document.addEventListener(
        "DOMContentLoaded",
        function () {
          reportPlatformSurface();
        },
        { once: true },
      );
    } else {
      reportPlatformSurface();
    }
  }
})();
