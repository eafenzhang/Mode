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
 * - `M1_EVENT_SUBSCRIBED`：M1 唯一接线的事件通道（深链），按载荷 `kind` 分流；
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
 * 成员清单不是手抄的「感觉」：`windowModeInjection.test.mts` 会拿
 * `packages/client/src/globals.d.ts`（`window.mode` 的类型声明真源）与
 * `@mode/ui/tauri-platform` 导出的 M1 切片逐名对拍，多一个/少一个/拼错一个都当场红。
 *
 * ## 依赖 `withGlobalTauri`
 *
 * 命令与事件都走 `window.__TAURI__`（`tauri.conf.json` 的 `app.withGlobalTauri: true`，
 * M1 的验收台也已依赖它）。**延迟解析**：Tauri 注入全局与用户 init script 的先后顺序不是本层
 * 能约定的，所以在脚本求值期**不**读它，只在方法被调用时才取——否则一个顺序差异就会让
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
 */
(function () {
  var NOT_IMPLEMENTED_PREFIX = "not_implemented_until_m3";
  var DEEP_LINK_EVENT = "mode:deep-link";
  var KIND_OAUTH = "oauth-callback";
  var KIND_PAYMENT = "payment-callback";

  /** M1 切片表的 10 条命令（与 `crates/platform/src/commands/mod.rs` 的切片表同名）。 */
  var M1_COMMANDS = [
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
  ];
  /** M1 通过**事件通道**实现的两条订阅方法（不在命令表里）。 */
  var M1_EVENT_SUBSCRIBED = ["onOAuthCallback", "onPaymentCallback"];
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
    "openExternalFile",
    "exportLogs",
    "getInstalledEditors",
    "openInEditor",
    "rdGetPeerCapability",
    "rdStartSession",
    "rdGetControlledState",
    "rdReadClipboardText",
    "rdGetServerState",
    "rdSetServerEnabled",
    "rdRevokeClient",
    "rdAuthorizeClient",
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
    "onUpdateReady",
    "onUpdateCheckResult",
    "onPostUpdateReleaseNotes",
    "acknowledgePostUpdateReleaseNotes",
    "quitAndInstallUpdate",
    "executeDesktopCommand",
    "setApplicationLocale",
    "setTitleBarTheme",
    "rdSendSignal",
    "rdEndSession",
    "onRemoteDesktopSessionEvent",
  ];
  /** 必选但**故意留空**的成员（理由见文件头「已知缺口」）。 */
  var DELIBERATELY_ABSENT_REQUIRED = ["log"];

  var diagnostics = {
    source: "sidecar-host/window_mode.ts",
    implemented: M1_COMMANDS.slice().concat(M1_EVENT_SUBSCRIBED),
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

  /** 延迟解析 Tauri 全局：注入那一刻它可能还没到（见文件头）。 */
  function tauriCore() {
    var api = window.__TAURI__;
    if (!api || !api.core || typeof api.core.invoke !== "function") return null;
    return api.core;
  }

  function invoke(command, args) {
    var core = tauriCore();
    // 不静默吞：壳没配好（`withGlobalTauri` 关着）必须是响亮的失败，而不是「命令返回空」。
    if (!core) throw new Error("mode_window_no_tauri_api: " + command);
    return core.invoke(command, args);
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

  /** 订阅深链事件里某一类 `kind`，返回同步的 disposer。 */
  function subscribeDeepLink(kind, callback) {
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
        if (typeof payload.url !== "string" || payload.url.length === 0) return;
        callback(payload.url);
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
    openInFileManager: function (path) {
      return invoke("openInFileManager", { path: path });
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
    onOAuthCallback: function (callback) {
      return subscribeDeepLink(KIND_OAUTH, callback);
    },
    onPaymentCallback: function (callback) {
      return subscribeDeepLink(KIND_PAYMENT, callback);
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
})();
