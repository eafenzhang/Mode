/**
 * 验收台上的 **Host 通道面板**（M3 Task 1 的可观察面）。
 *
 * 这一屏是「端口与启动状态真的到页面了吗」的肉眼证据：它只做**观测**，不替渲染层做任何判断——
 * 收到 `mode:service-port` 时校验 `event.source === window`、`event.ports.length`、
 * `event.ports[0] instanceof MessagePort`；收到 `mode:database-startup-state` 时显示
 * phase / startupId / sequence / errorCode；注入桥报错时显示它。
 *
 * 频道的两个字面量必须等于 `packages/shared/src/channels.ts` 的 `InternalChannels`：
 * 桥那侧（`crates/sidecar-host/src/bridge.ts`）与本文件由
 * `packages/desktop/tests/host-standalone.test.mts` 一起比对（改名即红）。
 *
 * 注意：本页面是 M1 的**命令面验收台**，不是产品 UI；Task 3 会把 `frontendDist` 换成真 renderer，
 * 那时这一屏作为开发期诊断面保留。
 */

/** 端口与状态流的频道名（与 `InternalChannels` 一致）。 */
const CHANNEL_SERVICE_PORT = "mode:service-port";
const CHANNEL_STARTUP_STATE = "mode:database-startup-state";

/** 注入桥挂上来的诊断面（`crates/sidecar-host/src/bridge.ts` 里定义）。 */
type HostBridgeState = {
  startupId?: string;
  portDelivered?: boolean;
  pumpFrames?: number;
  stateSequence?: number | null;
  phase?: string | null;
  error?: string | null;
  pumpStoppedReason?: string;
};

declare global {
  interface Window {
    __MODE_HOST_BRIDGE__?: HostBridgeState;
  }
}

type StartupState = {
  startupId?: string;
  sequence?: number;
  phase?: string;
  errorCode?: string;
  failedPhase?: string;
};

/** 把一条消息追加到面板（时间戳便于和宿主 stdout 对齐）。 */
function appendLine(out: HTMLElement, line: string): void {
  const stamp = new Date().toISOString().slice(11, 23);
  out.textContent = `${out.textContent ?? ""}[${stamp}] ${line}\n`;
}

/** 渲染面板并挂监听。返回一个刷新函数（供手动刷新桥诊断）。 */
export function renderHostChannelPanel(root: HTMLElement): () => void {
  const panel = document.createElement("section");
  panel.className = "deeplink";

  const heading = document.createElement("div");
  heading.className = "command-head";
  const name = document.createElement("span");
  name.className = "command-name";
  name.textContent = "Host 通道（M3 Task 1）";
  const summary = document.createElement("span");
  summary.className = "command-summary";
  summary.textContent = "端口 + database-startup 状态流：渲染层启动闸门的两条输入";
  heading.append(name, summary);

  const out = document.createElement("pre");
  out.className = "result";

  const refresh = (): void => {
    const bridge = window.__MODE_HOST_BRIDGE__;
    out.textContent = "";
    appendLine(
      out,
      bridge
        ? `注入桥：startupId=${bridge.startupId} 端口已投=${bridge.portDelivered} ` +
            `收帧=${bridge.pumpFrames ?? 0} 状态代=${bridge.stateSequence ?? "(未收到)"} ` +
            `phase=${bridge.phase ?? "(未收到)"}`
        : "注入桥：未见 __MODE_HOST_BRIDGE__（不是 Tauri 壳加载的页面，或注入脚本没跑）",
    );
    if (bridge?.error) appendLine(out, `注入桥错误：${bridge.error}`);
    if (bridge?.pumpStoppedReason) {
      appendLine(out, `帧泵已停：${bridge.pumpStoppedReason}`);
    }
  };

  window.addEventListener("message", (event: MessageEvent) => {
    // 这两条判定与渲染层的启动链同款：`source` 不是 window 的消息它根本不看。
    if (event.source !== window) return;

    if (event.data?.type === CHANNEL_STARTUP_STATE) {
      const state = event.data.state as StartupState | undefined;
      appendLine(
        out,
        `database-startup-state：phase=${state?.phase} startupId=${state?.startupId} ` +
          `sequence=${state?.sequence}` +
          (state?.errorCode ? ` errorCode=${state.errorCode} failedPhase=${state.failedPhase}` : ""),
      );
      return;
    }

    if (event.data?.type !== CHANNEL_SERVICE_PORT) return;
    const port = event.ports[0] as MessagePort | undefined;
    appendLine(
      out,
      `service-port：databaseStartupId=${event.data.databaseStartupId} ` +
        `event.source===window=${event.source === window} ` +
        `ports.length=${event.ports.length} 真 MessagePort=${port instanceof MessagePort}`,
    );
    // 旁听端口上的帧（不消费）：证明这个端口确实承载着 Host 的 RPC 字节。
    port?.addEventListener("message", (message: MessageEvent) => {
      const data = message.data as Uint8Array | undefined;
      appendLine(
        out,
        `端口帧：${Object.prototype.toString.call(data)} bytes=${data?.byteLength ?? "?"}`,
      );
    });
    port?.start();
  });

  window.addEventListener("mode:host-bridge-error", (event: Event) => {
    appendLine(out, `注入桥事件：${(event as CustomEvent<string>).detail}`);
  });

  panel.append(heading, out);
  root.append(panel);
  refresh();
  return refresh;
}
