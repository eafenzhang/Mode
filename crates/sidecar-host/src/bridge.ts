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
 * 命令名占位符由 Rust 侧替换（`crate::bridge::bridge_script`）。**startupId 不是占位符**：
 * 它在文档启动时经 `host_startup_id` 采用**当前代**——Host 自动重生会换代，reload 后的新文档
 * 必须绑定新 id（写死旧 id 会让换代保护拒收新代状态，闸门永远不放行）。
 * 频道名是字面量，由 `packages/desktop/tests/host-standalone.test.mts` 逐个比对
 * `InternalChannels` 的真实常量（改名即红）。
 */
(function () {
  var internals = window.__TAURI_INTERNALS__;
  /** 当前 Host 代（启动时由壳给出）。 */
  var STARTUP_ID = null;
  var CMD_SEND = "__MODE_HOST_CMD_SEND__";
  var CMD_RECV = "__MODE_HOST_CMD_RECV__";
  var CMD_STATE = "__MODE_HOST_CMD_STATE__";
  var CMD_STARTUP_ID = "__MODE_HOST_CMD_STARTUP_ID__";
  var CMD_STARTUP_CONTROL = "__MODE_HOST_CMD_STARTUP_CONTROL__";
  var CHANNEL_SERVICE_PORT = "mode:service-port";
  var CHANNEL_SCOPED_SERVICE_PORT = "mode:scoped-service-port";
  var CHANNEL_SCOPED_SERVICE_PORT_READY = "mode:scoped-service-port-ready";
  var CHANNEL_STARTUP_STATE = "mode:database-startup-state";
  var CHANNEL_STARTUP_CONTROL = "mode:database-startup-control";
  var SCOPED_FRAME_MARKER = 0xff;
  var CONTROL_FRAME_MARKER = 0xfe;
  var SCOPED_FRAME_HEADER_BYTES = 5;

  /** 页面可读的诊断面：验收台/集成测试用它看桥走到了哪一步。 */
  var bridgeState = {
    /** 启动时采用当前代后填充；此前为 null。 */
    startupId: null,
    portDelivered: false,
    /** 入方向（Host → 页面）搬运过的帧数。 */
    pumpFrames: 0,
    /**
     * 出方向（页面 → Host）搬运过的帧数。
     *
     * 与 `pumpFrames` 分开计数是因为两者的**语义不同**：入方向一上来就有 Host 的 `Initialize`，
     * 所以它非零不能说明渲染层发过东西；出方向非零才说明**页面侧**真的在这条端口上说话了
     * （真产品 UI 挂载后的 M3 Task 3 E2E 用它当业务流量证据）。
     */
    sentFrames: 0,
    /** 其中壳**收下**并且已经写进 Host stdin 的帧数（出方向端到端可用的强证据）。 */
    acceptedFrames: 0,
    /** LR-4：已注册的 remote scope id（诊断面）。 */
    scopedScopes: [],
    /** 附件控制帧（scope-ready / scope-detach）的发送次数。 */
    controlFrames: 0,
    /** 未知/已回收 scope 的入方向帧数（必须丢弃，不能误投 base）。 */
    droppedScopedFrames: 0,
    stateSequence: null,
    phase: null,
    error: null,
  };
  /** 最近一次投给渲染层的状态（`snapshot` 控制消息重新对账时再投一次）。 */
  var lastState = null;
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
  /** scopeId → 页面侧 scoped 端口的对壳端。 */
  var scopedPorts = {};
  /** attachmentId → scopeId 与反向表（renderer 的 ready ACK 只带 attachmentId）。 */
  var scopedAttachments = {};
  var scopedAttachmentIds = {};

  function encodeControlFrame(control) {
    var body = new TextEncoder().encode(JSON.stringify(control));
    var out = new Uint8Array(1 + body.byteLength);
    out[0] = CONTROL_FRAME_MARKER;
    out.set(body, 1);
    return out;
  }

  function sendControlFrame(control) {
    try {
      bridgeState.controlFrames += 1;
      internals.invoke(CMD_SEND, encodeControlFrame(control)).catch(function (error) {
        fail("host_control_send_failed", error);
      });
    } catch (error) {
      fail("host_control_send_threw", error);
    }
  }

  function isScopedFrame(bytes) {
    return bytes && bytes.byteLength >= SCOPED_FRAME_HEADER_BYTES && bytes[0] === SCOPED_FRAME_MARKER;
  }

  /** 读出 scope id 与去掉头部的 payload；调用方先过 isScopedFrame。 */
  function splitScopedFrame(bytes) {
    var view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    return {
      scopeId: view.getUint32(1, false),
      payload: bytes.subarray(SCOPED_FRAME_HEADER_BYTES),
    };
  }

  function encodeScopedFrame(scopeId, bytes) {
    var out = new Uint8Array(SCOPED_FRAME_HEADER_BYTES + bytes.byteLength);
    out[0] = SCOPED_FRAME_MARKER;
    new DataView(out.buffer, out.byteOffset, out.byteLength).setUint32(1, scopeId, false);
    out.set(bytes, SCOPED_FRAME_HEADER_BYTES);
    return out;
  }

  /** 出方向统一入口：base 原样、scoped 加前缀；计数与失败上报只有一份。 */
  function sendFrame(bytes) {
    try {
      bridgeState.sentFrames += 1;
      internals
        .invoke(CMD_SEND, bytes)
        .then(function () {
          // 壳收下了这一帧（已写进 Host 的 stdin）——出方向可用的**强**证据。
          bridgeState.acceptedFrames += 1;
        })
        .catch(function (error) {
          fail("host_send_failed", error);
        });
    } catch (error) {
      fail("host_send_threw", error);
    }
  }

  // 出方向：协议层发出来的本来就是 Uint8Array（VSBuffer.buffer），整包过 IPC 即可。
  shellPort.addEventListener("message", function (event) {
    sendFrame(event.data);
  });
  shellPort.start();

  /**
   * LR-4：为一个 remote scope 建页面端口。调用方（renderer）从 `remote.attach` 拿到
   * scopeId 与 attachment 元数据后调用；端口以既有 `mode:scoped-service-port` 消息投递，
   * renderer 的 `parseRemoteWorkspaceServicePortMessage` 不需要改形状。
   */
  function createScopedServicePort(options) {
    if (!options || typeof options !== "object") return false;
    var scopeId = options.scopeId;
    if (typeof scopeId !== "number" || scopeId % 1 !== 0 || scopeId <= 0) return false;
    if (scopedPorts[scopeId]) return false;
    var scopedChannel;
    try {
      scopedChannel = new MessageChannel();
    } catch (error) {
      fail("scoped_messagechannel_failed", error);
      return false;
    }
    scopedPorts[scopeId] = scopedChannel.port2;
    if (typeof options.attachmentId === "string" && options.attachmentId) {
      scopedAttachments[options.attachmentId] = scopeId;
      scopedAttachmentIds[scopeId] = options.attachmentId;
    }
    bridgeState.scopedScopes.push(scopeId);
    scopedChannel.port2.addEventListener("message", function (event) {
      if (!(event.data instanceof Uint8Array)) return;
      sendFrame(encodeScopedFrame(scopeId, event.data));
    });
    scopedChannel.port2.start();
    window.postMessage(
      {
        type: CHANNEL_SCOPED_SERVICE_PORT,
        attachmentId: options.attachmentId,
        sessionId: options.sessionId,
        target: options.target,
        capabilities: options.capabilities,
      },
      "*",
      [scopedChannel.port1],
    );
    return true;
  }

  function releaseScopedServicePort(scopeId) {
    var port = scopedPorts[scopeId];
    if (!port) return false;
    delete scopedPorts[scopeId];
    var attachmentId = scopedAttachmentIds[scopeId];
    if (attachmentId) {
      delete scopedAttachmentIds[scopeId];
      delete scopedAttachments[attachmentId];
    }
    bridgeState.scopedScopes = bridgeState.scopedScopes.filter(function (id) {
      return id !== scopeId;
    });
    sendControlFrame({ type: "scope-detach", scopeId: scopeId });
    try {
      port.close();
    } catch {
      /* 已关闭时忽略 */
    }
    return true;
  }

  bridgeState.createScopedServicePort = createScopedServicePort;
  bridgeState.releaseScopedServicePort = releaseScopedServicePort;

  /**
   * renderer 在 `MessagePort` 注册完成后 post 一条 ready ACK（`notifyRemoteWorkspaceServicePortReady`）。
   * 桥把它翻译成 Host 控制帧：Host 收到后才对 scoped `ChannelServer` 发 Initialize——
   * 提前发会被解复用按「未知 scope」丢弃，客户端永远停在未初始化。
   */
  window.addEventListener("message", function (event) {
    if (event.source !== window) return;
    var payload = event.data;
    if (!payload || typeof payload !== "object") return;
    if (payload.type !== CHANNEL_SCOPED_SERVICE_PORT_READY) return;
    var attachmentId = payload.attachmentId;
    if (typeof attachmentId !== "string" || !attachmentId) return;
    var scopeId = scopedAttachments[attachmentId];
    if (!scopeId) return;
    sendControlFrame({ type: "scope-ready", scopeId: scopeId });
  });

  // 入方向：长轮询。壳侧命令返回 raw bytes，页面拿到的是 ArrayBuffer。
  var pumpStopped = false;
  function deliverInbound(bytes) {
    if (isScopedFrame(bytes)) {
      var scoped = splitScopedFrame(bytes);
      var scopedPort = scopedPorts[scoped.scopeId];
      if (scopedPort) {
        scopedPort.postMessage(scoped.payload);
      } else {
        bridgeState.droppedScopedFrames += 1;
      }
      return;
    }
    shellPort.postMessage(bytes);
  }
  function pump() {
    if (pumpStopped) return;
    internals
      .invoke(CMD_RECV)
      .then(function (buffer) {
        if (buffer && buffer.byteLength > 0) {
          // 必须包一层 Uint8Array（spike §4.3）。
          deliverInbound(new Uint8Array(buffer));
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
        lastState = state;
        window.postMessage({ type: CHANNEL_STARTUP_STATE, state: state }, "*");
        setTimeout(pollState, 0);
      })
      .catch(function (error) {
        stateFailures += 1;
        fail("host_startup_state_failed", error);
        if (stateFailures <= MAX_STATE_FAILURES) setTimeout(pollState, 1000);
      });
  }

  /**
   * 终态屏（`GlobalDatabaseStartupLoading`）的启动控制：Electron main 的消费者已随旧线 EOL，
   * Tauri 线由壳接（`host_startup_control`）。retry = 壳清退避历史并重生 Host（随后整页
   * reload 绑定新代）；exit = 退出应用；snapshot = 把当前状态再投一次。
   */
  window.addEventListener("message", function (event) {
    if (event.source !== window) return;
    var payload = event.data;
    if (!payload || typeof payload !== "object") return;
    if (payload.type !== CHANNEL_STARTUP_CONTROL) return;
    var control = payload.control;
    var action = control && control.action;
    if (action === "snapshot") {
      if (lastState) {
        window.postMessage({ type: CHANNEL_STARTUP_STATE, state: lastState }, "*");
      }
      return;
    }
    if (action !== "retry" && action !== "exit" && action !== "kill") return;
    try {
      internals.invoke(CMD_STARTUP_CONTROL, { action: action }).catch(function (error) {
        fail("host_startup_control_failed", error);
      });
    } catch (error) {
      fail("host_startup_control_threw", error);
    }
  });

  /** 采用当前代 → 投端口 → 起状态流与泵。有界重试：IPC 一次性抖动不该让整条启动链白等 30s。 */
  var STARTUP_ID_ATTEMPTS = 3;
  var STARTUP_ID_RETRY_MS = 400;
  function adoptStartupId(attempt) {
    internals
      .invoke(CMD_STARTUP_ID)
      .then(function (startupId) {
        if (typeof startupId !== "string" || !startupId) {
          fail("host_startup_id_invalid", startupId);
          return;
        }
        STARTUP_ID = startupId;
        bridgeState.startupId = startupId;
        try {
          window.postMessage(
            { type: CHANNEL_SERVICE_PORT, databaseStartupId: STARTUP_ID },
            "*",
            [rendererPort],
          );
          bridgeState.portDelivered = true;
        } catch (error) {
          fail("deliver_port_failed", error);
          return;
        }
        // 端口先投、状态随后（渲染层的准入把两者分别存下，齐了才放行）。
        pollState();
        pump();
      })
      .catch(function (error) {
        if (attempt < STARTUP_ID_ATTEMPTS) {
          setTimeout(function () {
            adoptStartupId(attempt + 1);
          }, STARTUP_ID_RETRY_MS);
          return;
        }
        fail("host_startup_id_failed", error);
      });
  }

  function start() {
    adoptStartupId(1);
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", start, { once: true });
  } else {
    start();
  }
})();
