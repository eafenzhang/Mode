/**
 * harness 页（**测试资产**，不是产品前端）：只做一件事——用**渲染层同款**的接收形状拿端口，
 * 再让这个端口承载一次真实 `ChannelServer` 的 `Initialize` + 一次真实 RPC。
 *
 * 「同款接收形状」是逐字对齐的，且三处都指向**真实出处**（不是复刻实现）：
 * - `window.addEventListener("message", ...)` + 判定 `event.source === window`；
 * - 频道名取真实常量 `InternalChannels.ServicePort` / `DatabaseStartupState`（shared-shim 重定向到真文件）；
 * - 状态过真实 `databaseStartupStateSchema`；
 * - 准入用渲染层**未改动**的 `DatabaseStartupAdmission`（`@renderer-admission` alias → 真源码文件）；
 * - 连接层用真 `@mode/rpc` 的 `MessagePortProtocol` + `ChannelClient`。
 */
import { ChannelClient, MessagePortProtocol, type IChannel } from "@mode/rpc";
import { databaseStartupStateSchema, InternalChannels } from "@mode/shared";
import { DatabaseStartupAdmission } from "@renderer-admission";

declare global {
  interface Window {
    __TAURI_INTERNALS__: { invoke(cmd: string, payload?: unknown): Promise<unknown> };
    __SPIKE_LOG__: string[];
  }
}

const out = document.getElementById("out");
if (!out) throw new Error("缺少 #out");

// 走绝对地址的 fetch 自诊断（不经 Tauri invoke）：把「页面跑没跑」与「invoke 通不通」分开看。
const pageLog = (line: string): void => {
  void fetch(`http://127.0.0.1:5176/spike-log?page=${encodeURIComponent(line)}`).catch(() => {});
};
pageLog("main_module_loaded");

const log = (line: string): void => {
  const text = `[harness-page] ${line}`;
  window.__SPIKE_LOG__.push(text);
  console.log(text);
  pageLog(line);
  out.textContent = `${out.textContent ?? ""}${text}\n`;
};

window.__SPIKE_LOG__ = [];

const invoke = (cmd: string, payload?: unknown): Promise<unknown> =>
  window.__TAURI_INTERNALS__.invoke(cmd, payload);
const report = (payload: Record<string, unknown>): void => {
  log(`REPORT ${JSON.stringify(payload)}`);
  void invoke("spike_report", { line: JSON.stringify(payload) });
};

const withTimeout = async <T>(promise: Promise<T>, ms: number, what: string): Promise<T> => {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error(`${what} timeout ${ms}ms`)), ms);
      }),
    ]);
  } finally {
    if (timer) clearTimeout(timer);
  }
};

const admission = new DatabaseStartupAdmission();
let started = false;

function maybeRunRoundTrip(): void {
  const port = admission.takeReadyPort();
  if (!port || started) return;
  started = true;
  void runRoundTrip(port);
}

async function runRoundTrip(port: MessagePort): Promise<void> {
  log(
    `admission 交出端口：instanceof MessagePort=${port instanceof MessagePort} ctor=${
      Object.prototype.toString.call(port)
    }`,
  );
  try {
    const protocol = new MessagePortProtocol(port);
    const client = new ChannelClient(protocol);
    let initializeCount = 0;
    client.onDidInitialize(() => {
      initializeCount += 1;
      log(`ChannelClient 收到 Initialize（第 ${initializeCount} 次）`);
    });

    const result = await withTimeout(
      client.getChannel<IChannel>("spike").call<{
        command: string;
        echo: { n: number; text: string };
        hostPid: number;
        hostVersion: string;
      }>("ping", { n: 41, text: "from-page" }),
      15_000,
      "RPC ping",
    );

    report({
      status: "ok",
      portIsRealMessagePort: port instanceof MessagePort,
      initializeObserved: initializeCount > 0,
      rpcCommand: result.command,
      rpcEcho: result.echo,
      hostPid: result.hostPid,
      hostVersion: result.hostVersion,
    });
  } catch (error) {
    report({
      status: "failed",
      initializeObserved: false,
      message: error instanceof Error ? `${error.name}: ${error.message}` : String(error),
    });
  }
}

window.addEventListener("message", (event: MessageEvent) => {
  if (event.source !== window) return;

  if (event.data?.type === InternalChannels.DatabaseStartupState) {
    const parsed = databaseStartupStateSchema.safeParse(event.data.state);
    log(`收到 database-startup-state（schema 通过=${parsed.success}）`);
    if (!parsed.success) return;
    if (admission.acceptState(parsed.data)) maybeRunRoundTrip();
    return;
  }

  if (event.data?.type !== InternalChannels.ServicePort) return;
  const port = event.ports[0];
  log(
    `收到 service-port 消息：event.source===window=${event.source === window}` +
      `（不满足会被渲染层启动链直接忽略） event.ports.length=${event.ports.length}` +
      ` 真 MessagePort=${port instanceof MessagePort}`,
  );
  if (!port) return;
  // 观测：把页面上收到的**每一个**端口帧记下来（含类型），这是判定 ArrayBuffer/Uint8Array 陷阱的关键证据。
  port.addEventListener("message", (message: MessageEvent) => {
    log(`端口收到一帧：${Object.prototype.toString.call(message.data)} bytes=${message.data?.byteLength}`);
  });
  port.start();
  admission.acceptPort({ databaseStartupId: event.data.databaseStartupId }, port);
  maybeRunRoundTrip();
});

log("监听已就绪，等待注入桥送来端口与启动状态");
