/**
 * M3 Task 0 harness 的注入桥（**测试资产**，不是产品代码）。
 *
 * 这个文件由 Rust harness 用 `WebviewWindowBuilder::initialization_script` 注入，注入时机是
 * 「文档创建时、页面任何脚本之前」，注入顺序在 Tauri 自己的 invoke 系统之后
 * （tauri 2.12.4 `src/manager/webview.rs` 的 `prepare_pending_webview`：调用方脚本 append 在最后），
 * 所以这里能直接用 `window.__TAURI_INTERNALS__`。
 *
 * 两件事，都与渲染层既有代码兼容：
 * 1. 在页面内建 `MessageChannel`——**端口只能在页面内建**，Tauri/wry 没有「Rust 侧转移端口」的能力
 *    （见 spike 文档 §2.2）；`port1` 用 `window.postMessage(..., "*", [port1])` 交给渲染层，
 *    与 Electron preload 同一条路径，渲染层 `event.ports[0]` 拿到的是**真 MessagePort**；
 * 2. 把 `port2` 桥到壳：出方向 raw ArrayBuffer 的 invoke，入方向长轮询的 invoke 取回 raw bytes。
 *
 * 两个占位符由 Rust harness 在注入前替换：
 * - `__SPIKE_STARTUP_ID__`：端口负载的 `databaseStartupId` 与状态流的 `startupId`（必须同代）。
 * - `__SPIKE_WRAP_PORT_MESSAGE__`：`true` = 入方向包成 `Uint8Array`（正确）；
 *   `false` = 直接把 raw `ArrayBuffer` 投进端口——**这是 `MessagePortProtocol` 的静默丢帧陷阱**，
 *   harness 的 `no-wrap` 模式用它复现失败现场（spike 文档 §4.3）。
 *
 * 本文件被 `include_str!` 当**文本**注入，因此必须保持纯 JS 可执行（不加 TS 语法）；
 * 字面量频道名与 `packages/shared/src/channels.ts` 的 `InternalChannels` 保持一致，
 * 页面侧会用真常量断言它们相等（不一致就收不到消息）。
 */
(function () {
  var internals = window.__TAURI_INTERNALS__;
  var STARTUP_ID = "__SPIKE_STARTUP_ID__";
  var WRAP_PORT_MESSAGE = __SPIKE_WRAP_PORT_MESSAGE__;
  var LOG_ENDPOINT = "http://127.0.0.1:5176/spike-log";

  function bridgeLog(line) {
    try {
      void fetch(LOG_ENDPOINT + "?bridge=" + encodeURIComponent(String(line))).catch(function () {});
    } catch {
      /* 诊断通道本身失败就放弃 */
    }
  }

  bridgeLog(
    "injected_ran internals=" +
      typeof internals +
      " invoke=" +
      typeof (internals && internals.invoke) +
      " wrap=" +
      WRAP_PORT_MESSAGE,
  );

  function report(line) {
    bridgeLog("report " + String(line));
    try {
      internals.invoke("spike_report", { line: String(line) }).catch(function (error) {
        bridgeLog("report_invoke_failed " + String(error));
      });
    } catch (error) {
      bridgeLog("report_threw " + String(error));
    }
  }

  window.addEventListener("error", function (event) {
    bridgeLog("window_error " + (event.message || ""));
  });

  var channel;
  try {
    channel = new MessageChannel();
  } catch (error) {
    bridgeLog("messagechannel_failed " + String(error));
    return;
  }
  bridgeLog("messagechannel_created");

  var rendererPort = channel.port1; // 交给渲染层的那端
  var shellPort = channel.port2; // 留在桥里、对壳的一端

  // 出方向：MessagePortProtocol.send 发的本来就是 VSBuffer.buffer（Uint8Array），整包过 IPC 即可。
  shellPort.addEventListener("message", function (event) {
    bridgeLog("shell_port_recv " + Object.prototype.toString.call(event.data));
    internals.invoke("spike_host_send", event.data).catch(function (error) {
      report("shell_send_failed " + String(error));
    });
  });
  shellPort.start();

  // 入方向：长轮询。壳侧命令返回 raw bytes（Response::new），拿到就投给 port2。
  var pumpCount = 0;
  var pump = function () {
    pumpCount += 1;
    if (pumpCount === 1) bridgeLog("pump_started");
    internals
      .invoke("spike_host_recv")
      .then(function (buffer) {
        if (pumpCount === 1) {
          bridgeLog(
            "pump_first_frame " + Object.prototype.toString.call(buffer) + " bytes=" + (buffer && buffer.byteLength),
          );
        }
        if (buffer && buffer.byteLength > 0) {
          try {
            // 正确形态：包成 Uint8Array（MessagePortProtocol 的收帧判定是 `e.data instanceof Uint8Array`）。
            // 陷阱形态（WRAP_PORT_MESSAGE=false）：直接投 ArrayBuffer，协议层会静默丢弃。
            shellPort.postMessage(WRAP_PORT_MESSAGE ? new Uint8Array(buffer) : buffer);
            if (pumpCount === 1) bridgeLog("pump_first_frame_posted wrap=" + WRAP_PORT_MESSAGE);
          } catch (error) {
            bridgeLog("pump_post_threw " + String(error));
          }
        }
        setTimeout(pump, 0);
      })
      .catch(function (error) {
        bridgeLog("pump_failed " + String(error));
        report("shell_recv_failed " + String(error));
      });
  };
  pump();

  // 观测：从桥这一侧看到渲染层发往 Host 的帧（不消费，只是旁听）。
  rendererPort.addEventListener("message", function (event) {
    bridgeLog("bridge_seen_client_frame " + Object.prototype.toString.call(event.data));
  });

  function deliver() {
    bridgeLog("deliver_start readyState=" + document.readyState);
    try {
      window.postMessage(
        { type: "mode:service-port", databaseStartupId: STARTUP_ID },
        "*",
        [rendererPort],
      );
      bridgeLog("deliver_port_posted");
    } catch (error) {
      bridgeLog("deliver_port_threw " + String(error));
    }
    try {
      window.postMessage(
        {
          type: "mode:database-startup-state",
          state: {
            schemaVersion: 1,
            startupId: STARTUP_ID,
            attemptId: "harness-attempt-1",
            sequence: 0,
            startedAt: Date.now(),
            updatedAt: Date.now(),
            phase: "ready",
            disk: [],
          },
        },
        "*",
      );
      bridgeLog("deliver_state_posted");
    } catch (error) {
      bridgeLog("deliver_state_threw " + String(error));
    }
    report("delivered port+state");
  }

  if (document.readyState === "loading") {
    // 必须等 DOMContentLoaded：渲染层的 `window.addEventListener("message", ...)` 注册在**模块脚本**里，
    // 而 deferred 模块脚本在 DOMContentLoaded 之前执行完；提前投递会白丢一次（见 spike 文档 §4.5）。
    document.addEventListener("DOMContentLoaded", deliver, { once: true });
    bridgeLog("deliver_deferred_to_domcontentloaded");
  } else {
    deliver();
  }
})();
