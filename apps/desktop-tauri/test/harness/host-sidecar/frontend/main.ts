/**
 * opt-in 集成测试页（**测试资产**）：验「产品壳 + 产品 Host 能让渲染层的启动闸门放行」。
 *
 * 页面本身不实现任何业务：它逐字复刻渲染层启动链的**接收形状**，且三处都指向真实出处——
 * 频道名取真常量 `InternalChannels`、状态过真 schema `databaseStartupStateSchema`、
 * 准入用**未改动**的 `DatabaseStartupAdmission`（vite alias 到真源码文件）。
 * 与 Task 0 harness 的差别只有一处：这里的端口/状态来自**产品注入桥**（`crates/sidecar-host`），
 * 而且会顺带调一次真 RPC（`host.describe`）确认端口确实承载着产品 Host 的 `ChannelServer`。
 *
 * 报告走 `http://127.0.0.1:5177/host-roundtrip-log`（测试自己起的 sink）而不是 `console.log`：
 * WebView2 的 console 本机拿不到（M1 §4.3），而这条通道独立于 Tauri invoke——
 * invoke 全被拒时页面照样能报告它走到了哪一步。
 */
import { ChannelClient, MessagePortProtocol, type IChannel } from "@mode/rpc";
import { databaseStartupStateSchema, InternalChannels } from "@mode/shared";
import { DatabaseStartupAdmission } from "@renderer-admission";

declare global {
  interface Window {
    __MODE_HOST_BRIDGE__?: {
      startupId?: string;
      portDelivered?: boolean;
      pumpFrames?: number;
      stateSequence?: number | null;
      phase?: string | null;
      error?: string | null;
    };
  }
}

const LOG_ENDPOINT = "http://127.0.0.1:5177/host-roundtrip-log";
const GATE_TIMEOUT_MS = 20_000;
const RPC_TIMEOUT_MS = 15_000;

const out = document.getElementById("out");
if (!out) throw new Error("缺少 #out");

const lines: string[] = [];
let finished = false;

function log(line: string): void {
  lines.push(line);
  out.textContent = `${lines.join("\n")}\n`;
  void fetch(`${LOG_ENDPOINT}?line=${encodeURIComponent(line)}`).catch(() => {});
}

function report(payload: Record<string, unknown>): void {
  if (finished) return;
  finished = true;
  log(`REPORT ${JSON.stringify(payload)}`);
  void fetch(`${LOG_ENDPOINT}?report=${encodeURIComponent(JSON.stringify(payload))}`).catch(
    () => {},
  );
}

const withTimeout = async <T>(promise: Promise<T>, ms: number, what: string): Promise<T> => {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error(`${what} 超时 ${ms}ms`)), ms);
      }),
    ]);
  } finally {
    if (timer) clearTimeout(timer);
  }
};

const admission = new DatabaseStartupAdmission();
let started = false;
const seen = {
  port: false,
  portIsRealMessagePort: false,
  statePhase: null as string | null,
  stateStartupId: null as string | null,
  schemaAccepted: false,
};

log("page_loaded");

/** 注入桥报的 startupId（端口负载与状态流的同代判据都用它）。 */
function deliveredStartupId(): string | null {
  return window.__MODE_HOST_BRIDGE__?.startupId ?? null;
}

function maybeRun(): void {
  const port = admission.takeReadyPort();
  if (!port || started) return;
  started = true;
  void runRoundTrip(port);
}

async function runRoundTrip(port: MessagePort): Promise<void> {
  log(`gate_opened port_is_message_port=${port instanceof MessagePort}`);
  try {
    const client = new ChannelClient(new MessagePortProtocol(port));
    const initialize = new Promise<void>((resolveInit) => {
      client.onDidInitialize(() => resolveInit());
    });
    await withTimeout(initialize, GATE_TIMEOUT_MS, "Initialize");

    const describe = await withTimeout(
      client.getChannel<IChannel>("host").call<Record<string, unknown>>("describe"),
      RPC_TIMEOUT_MS,
      "host.describe",
    );
    log(`rpc_ok name=${String(describe.name)} pid=${String(describe.pid)}`);

    const delivered = deliveredStartupId();
    report({
      status: "ok",
      gateOpened: true,
      portIsRealMessagePort: port instanceof MessagePort,
      initializeObserved: true,
      schemaAccepted: seen.schemaAccepted,
      statePhase: seen.statePhase,
      stateStartupId: seen.stateStartupId,
      startupIdMatches: seen.stateStartupId !== null && seen.stateStartupId === delivered,
      describe,
    });
  } catch (error) {
    report({
      status: "failed",
      gateOpened: true,
      initializeObserved: false,
      message: error instanceof Error ? `${error.name}: ${error.message}` : String(error),
    });
  }
}

window.addEventListener("message", (event: MessageEvent) => {
  // 渲染层的启动链只看 `event.source === window` 的消息；这里逐字同款。
  if (event.source !== window) return;

  if (event.data?.type === InternalChannels.DatabaseStartupState) {
    const parsed = databaseStartupStateSchema.safeParse(event.data.state);
    seen.schemaAccepted = parsed.success;
    if (!parsed.success) {
      log(`state_schema_rejected ${JSON.stringify(parsed.error.issues.slice(0, 3))}`);
      return;
    }
    seen.statePhase = parsed.data.phase;
    seen.stateStartupId = parsed.data.startupId;
    log(`state phase=${parsed.data.phase} startupId=${parsed.data.startupId}`);
    if (parsed.data.phase === "failed") {
      // Task 2 的失败路径（DR-T1-3）：渲染层一旦收到任何状态就**拆掉自己的 30s 计时器**，
      // 所以 `failed` + `errorCode` 是此后唯一的失败信号 —— 收到就必须报告，不能只记一行日志
      // 然后等那个（对渲染层而言已经不存在了的）超时。`failedPhase` 一起报，因为渲染层的
      // 重试/退出判据读它。
      report({
        status: "startup_failed",
        gateOpened: false,
        portSeen: seen.port,
        portIsRealMessagePort: seen.portIsRealMessagePort,
        initializeObserved: false,
        schemaAccepted: seen.schemaAccepted,
        statePhase: parsed.data.phase,
        stateStartupId: parsed.data.startupId,
        failedPhase: parsed.data.failedPhase ?? null,
        errorCode: parsed.data.errorCode ?? null,
        startupIdMatches:
          seen.stateStartupId !== null && seen.stateStartupId === deliveredStartupId(),
      });
      return;
    }
    if (admission.acceptState(parsed.data)) maybeRun();
    return;
  }

  if (event.data?.type !== InternalChannels.ServicePort) return;
  const port = event.ports[0] as MessagePort | undefined;
  seen.port = true;
  seen.portIsRealMessagePort = port instanceof MessagePort;
  log(
    `port event.source===window=${event.source === window} ports.length=${event.ports.length}` +
      ` real_message_port=${port instanceof MessagePort}` +
      ` databaseStartupId=${String(event.data.databaseStartupId)}`,
  );
  if (!port) return;
  admission.acceptPort({ databaseStartupId: event.data.databaseStartupId }, port);
  maybeRun();
});

window.addEventListener("mode:host-bridge-error", (event: Event) => {
  log(`bridge_error ${(event as CustomEvent<string>).detail}`);
});

setTimeout(() => {
  report({
    status: "failed",
    gateOpened: false,
    portSeen: seen.port,
    portIsRealMessagePort: seen.portIsRealMessagePort,
    statePhase: seen.statePhase,
    schemaAccepted: seen.schemaAccepted,
    bridge: window.__MODE_HOST_BRIDGE__ ?? null,
    message: `${GATE_TIMEOUT_MS}ms 内闸门没有放行`,
  });
}, GATE_TIMEOUT_MS);
