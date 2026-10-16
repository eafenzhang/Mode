/**
 * 真产品 UI 的 opt-in 集成测试**探针**（M3 Task 3 · Step 4）——**测试资产**，不是产品代码。
 *
 * 它由 `ProductUiHarness` 用 `WebviewWindowBuilder::initialization_script` **追加**在 Host 桥之后
 * 注入（见 `apps/desktop-tauri/src-tauri/src/window.rs` 的 `extra_init_script`）。为什么要有这一层：
 *
 * - 页面是 **renderer 的构建产物**（`packages/desktop/out/renderer`），本任务**不许改 renderer 源码**，
 *   所以「页面里塞一个探针」这条路不通；注入才是唯一能观测它的位置；
 * - 注入的先后是确定的：桥（含 `window.mode`）先、本探针后，而**页面脚本都还没开始**
 *   （`type="module"` 是 deferred）。所以本探针在 **document-start** 读到 `window.mode`
 *   就等于「renderer 第一次有机会读它时它已经在」——这正是 DR-T3-1 那条顺序性质，
 *   而且是在**真页面、真 webview**上读的（CI 里的等价断言在
 *   `apps/desktop-tauri/test/windowModeInjection.test.mts`，那边是同一段脚本的替身环境）。
 *
 * 报告走 `http://127.0.0.1:5177/…`（测试自己起的 sink）：WebView2 的 console 本机拿不到
 * （M1 §4.3），而这条通道独立于 Tauri invoke。
 *
 * 「业务面走通」在这份探针里被拆成三个**互相独立**的可观察事实（缺一条就不算通）：
 *
 * 1. `mounted`：`#root` 里有 React 渲染出来的子树——渲染层入口模块跑完了；
 * 2. `acceptedFrames > 0`：**页面侧**真的往端口上发了帧，而且壳收下并写进了 Host 的 stdin
 *    （`bridge.ts` 的 `sentFrames`/`acceptedFrames`，出方向的两个计数）；
 * 3. `pumpFramesGrewAfterReady`：闸门放行之后**入方向还在增长**——Host 只会对渲染层的请求作答，
 *    所以这等于「渲染层的请求真的到了真服务并拿到了应答」。
 *
 * 另外带上 `uiText` 的片段：它是渲染层真实产物的内容，交给测试人眼复核（原始输出入仓）。
 */
(function () {
  var LOG_ENDPOINT = "http://127.0.0.1:5177/product-ui-log";
  var PROBE = {
    modeAtDocumentStart: typeof window.mode,
    modeKeyCountAtDocumentStart: 0,
    modeDiagnosticsAtDocumentStart: null,
  };

  function bridge() {
    return window.__MODE_HOST_BRIDGE__ || null;
  }

  function log(line) {
    try {
      void fetch(LOG_ENDPOINT + "?line=" + encodeURIComponent(line)).catch(function () {});
    } catch (error) {
      void error;
    }
  }

  function report(payload) {
    try {
      void fetch(LOG_ENDPOINT + "?report=" + encodeURIComponent(JSON.stringify(payload))).catch(
        function () {},
      );
    } catch (error) {
      void error;
    }
  }

  // ① document-start 读一次：先读、后断言。此刻页面的 module 脚本还没开始执行。
  try {
    if (window.mode) {
      PROBE.modeKeyCountAtDocumentStart = Object.keys(window.mode).length;
    }
    PROBE.modeDiagnosticsAtDocumentStart = window.__MODE_WINDOW_MODE__ ? {
      implemented: window.__MODE_WINDOW_MODE__.implemented.length,
      notImplemented: window.__MODE_WINDOW_MODE__.notImplemented.length,
      deliberatelyAbsent: window.__MODE_WINDOW_MODE__.deliberatelyAbsent,
    } : null;
  } catch (error) {
    PROBE.probeError = String(error);
  }

  /** 渲染层挂载的可观察形态：`#root` 里有子树。 */
  function mounted() {
    var root = document.getElementById("root");
    return !!root && root.childElementCount > 0;
  }

  function uiText() {
    var root = document.getElementById("root");
    if (!root) return "";
    var text = (root.innerText || root.textContent || "").replace(/\s+/g, " ").trim();
    return text.slice(0, 400);
  }

  var started = Date.now();
  var readyFrameWatermark = null;
  var reported = false;

  function finish(status, extra) {
    if (reported) return;
    reported = true;
    var state = bridge();
    var payload = {
      status: status,
      modeAtDocumentStart: PROBE.modeAtDocumentStart,
      modeKeyCountAtDocumentStart: PROBE.modeKeyCountAtDocumentStart,
      modeDiagnosticsAtDocumentStart: PROBE.modeDiagnosticsAtDocumentStart,
      probeError: PROBE.probeError || null,
      rendererStarted: typeof window.__MODE_RENDERER_START__ === "number",
      mounted: mounted(),
      eagerPlatformCreated: typeof window.mode === "object",
      startupId: state ? state.startupId : null,
      portDelivered: state ? state.portDelivered : null,
      phase: state ? state.phase : null,
      pumpFrames: state ? state.pumpFrames : null,
      sentFrames: state ? state.sentFrames : null,
      acceptedFrames: state ? state.acceptedFrames : null,
      readyFrameWatermark: readyFrameWatermark,
      pumpFramesGrewAfterReady:
        readyFrameWatermark === null || !state ? false : state.pumpFrames > readyFrameWatermark,
      bridgeError: state ? state.error : null,
      // 完整降级清单（注入层按「理由」去重，所以它是有界的）：**不截断**——
      // 截断会让「只检查前 N 条」的断言变成半盲的。
      degraded: window.__MODE_WINDOW_MODE__ ? window.__MODE_WINDOW_MODE__.degraded.slice() : null,
      uiText: uiText(),
      elapsedMs: Date.now() - started,
    };
    if (extra) {
      for (var key in extra) {
        if (Object.prototype.hasOwnProperty.call(extra, key)) payload[key] = extra[key];
      }
    }
    log("PROBE_REPORT " + JSON.stringify(payload));
    report(payload);
  }

  log("probe_document_start mode=" + PROBE.modeAtDocumentStart + " keys=" + PROBE.modeKeyCountAtDocumentStart);

  // ② 定一个就绪水位：闸门放行（phase=ready）那一刻的入方向帧数。
  //    两件事**各自独立**地等：就绪水位（看业务应答）与挂载（看真产品 UI）。
  //    早先把「挂载」当成收工条件，结果 React 一提交就把轮询停了，水位永远没取到。
  var watermarkTimer = setInterval(function () {
    var state = bridge();
    if (state && state.phase === "ready" && readyFrameWatermark === null) {
      readyFrameWatermark = state.pumpFrames;
      log(
        "ready_frame_watermark pumpFrames=" +
          readyFrameWatermark +
          " sentFrames=" +
          state.sentFrames,
      );
    }
    if (mounted() && readyFrameWatermark !== null) {
      clearInterval(watermarkTimer);
      // 挂载之后再给一段静默期：业务 RPC 往往在 React 提交之后才发出来。
      setTimeout(function () {
        finish("ok");
      }, 6000);
    }
  }, 250);

  // ③ 兜底：到点如实报告（挂不上也必须有原始输出，而不是只留一句「超时」）。
  setTimeout(function () {
    clearInterval(watermarkTimer);
    finish("timeout");
  }, 30000);
})();
