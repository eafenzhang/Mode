/**
 * Mode Host sidecar 的**独立进程入口**（无 Electron）。
 *
 * M3 Task 1 建了这条链路（壳 → Host → 端口 → 渲染层）但服务面是 stub；**Task 2 把 stub 换成
 * `packages/services` 的真服务面**，并让 Host 的就绪信号真正由「真实启动」驱动。契约与验收分工：
 * `docs/specs/tauri-renderer-host.md`。
 *
 * ## 启动序列（真实工作，不再是合成的就绪）
 *
 * ```
 * starting                  ← 状态机初值（还什么都没做）
 *   preparing_host_storage  ← 真开任务索引 sqlite：真报告 checking/waiting_for_lock/
 *                              migrating/maintaining/committing/ready 与迁移事实
 *   starting_services       ← createLocalServices（真服务面装配）
 * ready                     ← 服务频道全部注册进 ChannelServer 之后
 * ```
 *
 * 任一步抛出 → `failed` + `errorCode`（`classifyDatabaseStartupError`，不自己编分类），
 * 结构化写 stderr，然后退出：壳的 reaper 会把它变成推给渲染层的
 * `failed(transport_closed)`（**渲染层在收到第一条状态后没有自己的超时**，所以失败必须由外面推）。
 * 精确错码与停在哪个阶段在 stderr 与 `host.describe().startup` 上——把分段阶段合并进
 * 渲染层可见的状态流需要一条 Host → 壳 的新通道，本任务不改传输层，差额登记在 spec 里。
 *
 * ## 三条边界上的守卫（都由 Task 1 的实测/本任务的实测支持）
 *
 * 1. **stdout 只用于帧**：真服务在进程内 `console.log`（实测 `cua-pip-session`），
 *    混进长度前缀帧流会让帧边界错位 → `stdoutGuard.ts` 在装载服务**之前**接管 stdout；
 * 2. **`listen` 抛错不能杀死 Host**：真服务面由 `ProxyChannel.fromService` 包装，未知事件名
 *    直接抛，而 `ChannelServer.onEventListen` 没有 try/catch → `channelGuard.ts` 把守卫装在
 *    `registerChannel` 与派发入口两处（一次覆盖全部服务）；
 * 3. **帧形状**：`ChannelServer` 假设对端只发合法帧（spike §4.6 实测崩溃）→ 入口先校验。
 *
 * 环境变量（壳负责设置，见 spec 的「环境契约」）：
 * - `MODE_HOST_STARTUP_ID`：本次 Host 实例的启动代 id；省略时按 pid 兜底生成（仅便于裸跑）。
 * - `MODE_DATA_BASE_DIR`：数据**基**目录覆写（不是数据根）；`HOME` / `USERPROFILE`：家目录兜底。
 *   数据根 = `{base}/.mode`，见 `./dataRoot.js`。
 * - `MODE_HOST_NATIVE_DEP_PROBE`：设为 `spawn` 时原生依赖探针会真起一个 pty（测试用）。
 * - `MODE_BUILTIN_PROVIDER_CONFIG_FILE` / `MODE_HOST_RESOURCES_DIR`：Built-in Provider Config 定位。
 *
 * stdout **只用于帧**（由 `stdoutGuard` 强制执行）；任何日志都走 stderr。
 */
import {
  ChannelServer,
  Emitter,
  VSBuffer,
  type IMessagePassingProtocol,
  type IServerChannel,
} from "@mode/rpc";

import { DispatchGuards } from "./channelGuard.js";
import { resolveDataRoot } from "./dataRoot.js";
import {
  MODE_HOST_NATIVE_DEP_PROBE_ENV,
  probeNativeDeps,
  probePtySpawn,
  type NativeDepsReport,
} from "./nativeDeps.js";
import { createHostServiceSurface, type HostServiceSurface } from "./serviceAccessor.js";
import { StartupStateMachine } from "./startupMapping.js";
import { StdoutGuard } from "./stdoutGuard.js";
import { createFrameDecoder, isRpcRequestFrame } from "./stdioFrames.js";

/** 诊断频道名：不属于 `ServiceChannels`，只报 Host 自身的身份与进程事实（只读，无副作用）。 */
export const HOST_CHANNEL = "host";

/** 第一件事：接管 stdout。任何在此之后装载的模块写到 stdout 的字节都会被转到 stderr。 */
const stdoutGuard = new StdoutGuard();

const log = (line: string): void => {
  process.stderr.write(`[mode-host-sidecar] ${line}\n`);
};

const startupId = process.env.MODE_HOST_STARTUP_ID?.trim() || `host-${process.pid}`;
const dataRoot = resolveDataRoot(process.env);
const guards = new DispatchGuards();
const startup = new StartupStateMachine({
  startupId,
  attemptId: `attempt-${startupId}`,
});

/** 已发布的应用阶段历史（诊断用；有界）。 */
const startupHistory: string[] = [];
function recordStartupState(): void {
  const state = startup.snapshot();
  startupHistory.push(`${state.sequence}:${state.phase}`);
  if (startupHistory.length > 32) startupHistory.shift();
}

/** 原生依赖探针（DR-T2-1）。默认档同步；`=spawn` 档真起一个 pty（测试用）。 */
let nativeDeps: NativeDepsReport = probeNativeDeps(process.env);

/** 真服务面（装配成功后才有；失败路径下保持 `undefined` 并在诊断里说明）。 */
let serviceSurface: HostServiceSurface | undefined;

const messages = new Emitter<VSBuffer>();
const protocol: IMessagePassingProtocol = {
  // 出方向：唯一的帧写入口（走 stdout 守卫的原始写函数，不经非帧判定）。
  send: (buffer) => stdoutGuard.writeFrameBytes(buffer.buffer),
  onMessage: messages.event,
};

let rejectedFrames = 0;
process.stdin.on(
  "data",
  createFrameDecoder(
    (frame) => {
      // 形状校验必须在这里：`ChannelServer` 假设对端只发合法帧，投坏帧会让 Host 进程崩
      // （spike §4.6 的实测崩溃）。坏帧只记账并留痕，绝不进协议层。
      if (!isRpcRequestFrame(frame)) {
        rejectedFrames += 1;
        log(`frame_rejected bytes=${frame.byteLength} firstByte=${frame[0] ?? "(none)"}`);
        return;
      }
      // 派发守卫：协议层/序列化层的任何同步异常都只被记账，不再变成带走进程的未捕获异常。
      guards.dispatch("rpc_dispatch", () => messages.fire(VSBuffer.wrap(frame)), log);
    },
    (reason) => log(`frame_decoder_rejected ${reason}`),
  ),
);

let shuttingDown = false;
process.stdin.on("end", () => {
  shuttingDown = true;
  log("stdin_closed_exit");
  // 先释放真服务（后台任务/子进程/句柄），再退出；释放失败也必须退出（否则留孤儿）。
  void (serviceSurface?.dispose() ?? Promise.resolve())
    .catch((error: unknown) => log(`dispose_failed ${describeError(error)}`))
    .finally(() => process.exit(0));
});
process.stdin.on("error", (error: Error) => {
  log(`stdin_error ${error.message}`);
});

function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** `host.describe` 的载荷（同一份类型也贴在测试里：跨语言契约只看字段名）。 */
interface HostDescribePayload {
  name: string;
  protocolVersion: number;
  pid: number;
  nodeVersion: string;
  platform: string;
  arch: string;
  startupId: string;
  dataBaseDir: string;
  dataRoot: string;
  dataBaseDirSource: string;
  rejectedFrames: number;
  serviceChannels: string[];
  serviceSurface: {
    kind: "services" | "unavailable";
    channelCount: number;
    builtinProviderConfigPath: string;
    builtinProviderConfigSource: string;
    builtinProviderConfigExists: boolean;
    guardedSubscriptions: number;
    dispatchGuards: number;
    guardSamples: string[];
    stdoutDiverted: number;
    stdoutSamples: string[];
  };
  nativeDeps: NativeDepsReport;
  startup: {
    phase: string;
    sequence: number;
    attemptId: string;
    failedPhase?: string;
    errorCode?: string;
    databasePhase?: string;
    history: string[];
  };
}

function describeHost(): HostDescribePayload {
  const guardSnapshot = guards.snapshot();
  const stdoutSnapshot = stdoutGuard.snapshot();
  const state = startup.snapshot();
  return {
    name: "mode-host-sidecar",
    protocolVersion: 2,
    pid: process.pid,
    nodeVersion: process.version,
    platform: process.platform,
    arch: process.arch,
    startupId,
    // 两个都给：`dataBaseDir` 是 MODE_DATA_BASE_DIR 的语义（基目录），
    // `dataRoot` 是真数据根（= base/.mode）。只报一个正是当初把口径搞反的原因。
    dataBaseDir: dataRoot.dataBaseDir,
    dataRoot: dataRoot.dataRoot,
    dataBaseDirSource: dataRoot.source,
    rejectedFrames,
    serviceChannels: serviceSurface?.channelNames ?? [],
    serviceSurface: {
      kind: serviceSurface ? "services" : "unavailable",
      channelCount: serviceSurface?.channelNames.length ?? 0,
      builtinProviderConfigPath: serviceSurface?.builtinProviderConfig.path ?? "",
      builtinProviderConfigSource: serviceSurface?.builtinProviderConfig.source ?? "",
      builtinProviderConfigExists: serviceSurface?.builtinProviderConfig.exists ?? false,
      guardedSubscriptions: guardSnapshot.guardedSubscriptions,
      dispatchGuards: guardSnapshot.dispatchGuards,
      guardSamples: guardSnapshot.samples,
      stdoutDiverted: stdoutSnapshot.diverted,
      stdoutSamples: stdoutSnapshot.samples,
    },
    nativeDeps,
    startup: {
      phase: state.phase,
      sequence: state.sequence,
      attemptId: state.attemptId,
      ...(state.failedPhase ? { failedPhase: state.failedPhase } : {}),
      ...(state.errorCode ? { errorCode: state.errorCode } : {}),
      ...(state.databasePhase ? { databasePhase: state.databasePhase } : {}),
      history: [...startupHistory],
    },
  };
}

/** 诊断频道：只报事实，不做任何业务动作（M1 的 smoke 吃过「没法观察渲染层」的亏）。 */
const hostChannel: IServerChannel<string> = {
  // `IServerChannel.call<T>` 是泛型的（客户端按自己的期望定型），实现只能把已知载荷
  // 断言成 T —— 断言收在这一处，不在每个分支里撒。
  async call<T>(_ctx: string, command: string): Promise<T> {
    if (command === "describe") return describeHost() as T;
    throw new Error(`unknown command ${command}`);
  },
  listen(_ctx, event) {
    throw new Error(`host channel 没有事件：${event}`);
  },
};

/** 真服务面装配 + 频道注册 + 就绪。任一步抛出都由调用方转成 `failed` 并退出。 */
async function bootstrap(): Promise<ChannelServer> {
  // ① 原生依赖（DR-T2-1 的第一件事：判错就要重开 DR-M3-1）。
  if (process.env[MODE_HOST_NATIVE_DEP_PROBE_ENV]?.trim() === "spawn") {
    nativeDeps = { ...nativeDeps, pty: await probePtySpawn() };
  }
  log(
    `native_deps sqlite=${nativeDeps.sqlite.ok ? `ok(${nativeDeps.sqlite.version ?? "?"})` : "FAIL"}` +
      ` pty=${nativeDeps.pty.ok ? `ok(spawnVerified=${String(nativeDeps.pty.spawnVerified)})` : "FAIL"}` +
      ` cua=${nativeDeps.cua.injected ? `injected(${String(nativeDeps.cua.ok)})` : "not-injected"}`,
  );

  // ② 真存储：任务索引（应用阶段 preparing_host_storage，i18n 文案「准备任务索引」）。
  //    用公共入口 `@mode/services/storage-startup` 的 `prepareTasksIndexStorage`：
  //    它在进程内真开 `DatabaseSync`、真跑迁移，并按库内阶段回调（checking/waiting_for_lock/
  //    migrating/maintaining/committing/ready + 迁移事实）——这就是「真 startup」的来源。
  const { getTasksIndexDatabasePath, prepareTasksIndexStorage } = await import(
    "@mode/services/storage-startup"
  );
  startup.report("preparing_host_storage", {
    databaseId: "tasks-index",
    databaseKind: "tasks-index",
    databasePhase: "checking",
  });
  recordStartupState();
  const tasksIndexPath = getTasksIndexDatabasePath();
  await prepareTasksIndexStorage(tasksIndexPath, (phase, migration) => {
    startup.report("preparing_host_storage", {
      databaseId: "tasks-index",
      databaseKind: "tasks-index",
      databasePhase: phase,
      ...(migration ? { migration } : {}),
    });
    recordStartupState();
  });
  log(`storage_prepared path=${tasksIndexPath}`);

  // ③ 真服务面（应用阶段 starting_services）。
  startup.report("starting_services");
  recordStartupState();
  const surface = await createHostServiceSurface({ guards, log });
  serviceSurface = surface;
  log(
    `services_ready channels=${surface.channelNames.length}` +
      ` builtinConfig=${surface.builtinProviderConfig.source}(exists=${String(surface.builtinProviderConfig.exists)})`,
  );

  // ④ `deferInit = true`：先把频道注册齐再 ready()，否则 Initialize 可能抢在 registerChannel 之前
  //    （与 Task 0 的 Host 替身同一理由）。真服务频道与诊断频道都过守卫。
  const server = new ChannelServer(protocol, "mode-host-sidecar", 1000, true);
  for (const { channelName, channel } of surface.channels) {
    if (channelName === HOST_CHANNEL) {
      // 频道名冲突会让诊断频道被服务面顶掉，属于配置错误：立刻失败，不留半可用状态。
      throw new Error(`诊断频道名 ${HOST_CHANNEL} 与 ServiceChannels 冲突`);
    }
    server.registerChannel(channelName, channel);
  }
  server.registerChannel(HOST_CHANNEL, guards.guardChannel(HOST_CHANNEL, hostChannel, log));

  startup.report("ready");
  recordStartupState();
  server.ready();
  return server;
}

try {
  await bootstrap();
  log(
    `ready startupId=${startupId} dataBaseDir=${dataRoot.dataBaseDir} dataRoot=${dataRoot.dataRoot}` +
      ` source=${dataRoot.source} channels=${serviceSurface?.channelNames.length ?? 0}` +
      ` node=${process.version} shutdown=${String(shuttingDown)}`,
  );
} catch (error) {
  const failed = startup.fail(error);
  recordStartupState();
  // 结构化留痕（壳侧看不到 RPC 之前的状态，这条是排障的唯一入口）。
  process.stderr.write(
    `[mode-host-sidecar] startup_failed phase=${failed.phase}` +
      ` failedPhase=${failed.failedPhase ?? "?"} errorCode=${failed.errorCode ?? "?"}` +
      ` error=${describeError(error)}\n`,
    () => {
      // 退出让壳的 reaper 立刻推 `failed(transport_closed)`：渲染层收到第一条状态后没有自己的
      // 超时，所以「失败必须被外面推」这条硬要求只能由壳满足（spec §3 的 DR-T1-3）。
      process.exit(1);
    },
  );
}
