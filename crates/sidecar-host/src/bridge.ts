/**
 * 注入到 webview 的 Host 桥（**产品代码**，由 `crate::bridge` 用 `include_str!` 当文本注入，
 * 因此这里必须是纯 JS 可执行的语法，不能出现 TS 类型）。
 *
 * 它做三件事，对应 M3 Task 0 的 spike 结论（`docs/plans/2026-10-16-m3-port-transport-spike.md` §4）：
 *
 * 1. **端口只能在页面内建**：Tauri/wry 没有「Rust 侧造/转移端口」的能力，所以这里 `new MessageChannel()`，
 *    `port1` 用 `window.postMessage(payload, "*", [port1])` 交给渲染层——与 Electron preload 同一条路径，
 *    渲染层 `event.ports[0]` 拿到的是**真 MessagePort**，启动链一行都不用改（`event.source === window`
 *    这条判定也因此成立，`dispatchEvent` 做不到）；
 * 2. 把 `port2` 桥回壳：出方向 `invoke(host_send, frame)` 走 raw body；入方向长轮询 `invoke(host_recv)`，
 *    拿到的 **`ArrayBuffer` 必须包成 `Uint8Array`** 再投给端口——协议层的收帧判定是
 *    `e.data instanceof Uint8Array`，直接投 `ArrayBuffer` 会被**静默丢弃**（spike §4.3 的失败现场）；
 * 3. 把壳发布的状态流投给渲染层：`invoke(host_startup_state, { since })` 长轮询，
 *    状态一变就 `window.postMessage`。端口与状态都只在 `DOMContentLoaded` 之后投递——
 *    渲染层的 `window.addEventListener("message", …)` 注册在模块脚本里，提前投递会白丢一次（spike §4.5）。
 *
 * 三个占位符由 Rust 侧替换（`crate::bridge::bridge_script`）：startupId 与三个命令名。
 * 两个频道名是字面量，由 `packages/desktop/tests/host-standalone.test.mts` 逐个比对
 * `InternalChannels` 的真实常量（改名即红）。
 */
(function () {
  var internals = window.__TAURI_INTERNALS__;
  var STARTUP_ID = "__MODE_HOST_STARTUP_ID__";
  var CMD_SEND = "__MODE_HOST_CMD_SEND__";
  var CMD_RECV = "__MODE_HOST_CMD_RECV__";
  var CMD_STATE = "__MODE_HOST_CMD_STATE__";
  var CHANNEL_SERVICE_PORT = "mode:service-port";
  var CHANNEL_STARTUP_STATE = "mode:database-startup-state";

  /** 页面可读的诊断面：验收台/集成测试用它看桥走到了哪一步。 */
  var bridgeState = {
    startupId: STARTUP_ID,
    portDelivered: false,
    pumpFrames: 0,
    stateSequence: null,
    phase: null,
    error: null,
  };
  window.__MODE_HOST_BRIDGE__ = bridgeState;

  function fail(stage, error) {
    var text = stage + ": " + String(error);
    bridgeState.error = text;
    try {
      window.dispatchEvent(new CustomEvent("mode:host-bridge-error", { detail: text }));
    } catch {
      /* 诊断事件本身失败就放弃 */
    }
  }

  if (!internals || typeof internals.invoke !== "function") {
    fail("no_tauri_internals", "window.__TAURI_INTERNALS__.invoke 不可用");
    return;
  }

  var channel;
  try {
    channel = new MessageChannel();
  } catch (error) {
    fail("messagechannel_failed", error);
    return;
  }
  var rendererPort = channel.port1; // 交给渲染层的那端
  var shellPort = channel.port2; // 留给桥、对壳的一端

  // 出方向：协议层发出来的本来就是 Uint8Array（VSBuffer.buffer），整包过 IPC 即可。
  shellPort.addEventListener("message", function (event) {
    try {
      internals.invoke(CMD_SEND, event.data).catch(function (error) {
        fail("host_send_failed", error);
      });
    } catch (error) {
      fail("host_send_threw", error);
    }
  });
  shellPort.start();

  // 入方向：长轮询。壳侧命令返回 raw bytes，页面拿到的是 ArrayBuffer。
  var pumpStopped = false;
  function pump() {
    if (pumpStopped) return;
    internals
      .invoke(CMD_RECV)
      .then(function (buffer) {
        if (buffer && buffer.byteLength > 0) {
          // 必须包一层 Uint8Array（spike §4.3）。
          shellPort.postMessage(new Uint8Array(buffer));
          bridgeState.pumpFrames += 1;
        }
        if (!pumpStopped) setTimeout(pump, 0);
      })
      .catch(function (error) {
        // Host 传输没了：状态流会给出失败原因，泵自己停下（失败上报的主路径是状态流，不是这里）。
        pumpStopped = true;
        bridgeState.pumpStoppedReason = String(error);
      });
  }

  var MAX_STATE_FAILURES = 5;
  var stateFailures = 0;
  function pollState() {
    internals
      .invoke(CMD_STATE, { since: bridgeState.stateSequence })
      .then(function (state) {
        stateFailures = 0;
        if (!state || state.sequence === bridgeState.stateSequence) {
          setTimeout(pollState, 0);
          return;
        }
        bridgeState.stateSequence = state.sequence;
        bridgeState.phase = state.phase;
        if (state.startupId !== STARTUP_ID) {
          // 换代保护：不是本次启动的状态，绝不投给渲染层（投了会让它按错代放行）。
          fail("startup_id_mismatch", state.startupId);
          setTimeout(pollState, 0);
          return;
        }
        window.postMessage({ type: CHANNEL_STARTUP_STATE, state: state }, "*");
        setTimeout(pollState, 0);
      })
      .catch(function (error) {
        stateFailures += 1;
        fail("host_startup_state_failed", error);
        if (stateFailures <= MAX_STATE_FAILURES) setTimeout(pollState, 1000);
      });
  }

  function start() {
    try {
      window.postMessage(
        { type: CHANNEL_SERVICE_PORT, databaseStartupId: STARTUP_ID },
        "*",
        [rendererPort],
      );
      bridgeState.portDelivered = true;
    } catch (error) {
      fail("deliver_port_failed", error);
    }
    // 端口先投、状态随后（渲染层的准入把两者分别存下，齐了才放行）。
    pollState();
    pump();
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", start, { once: true });
  } else {
    start();
  }
})();
