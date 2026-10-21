/* eslint-disable max-lines -- 被控 host 侧会话/设置/注入集中维护同一套单会话互斥与收口纪律，拆散会让收口路径分散。 */
/**
 * 远程桌面**被控 host 侧**协调（docs/specs/lan-remote-desktop.md 事件顺序 ②③⑤⑥）。
 *
 * 端口方向图（本模块是 host 侧唯一入口）：
 *
 *   lanAccessServer ──(hook)requestSession(clientId)──────► 本模块 ──ConsentRequest──► 被控 main
 *   WS /ws/remote-desktop?ticket ◄──(hook)onSessionSocket── 本模块 ◄─ConsentResult─── 被控 main
 *        │ 收到的每一帧 ──────────────────────────────────► ──SignalFromHost──► main
 *        │ ◄──────────────────────────────────────────────── ◄─SignalToHost──── main
 *        └─ 断开/bye ─────────────────────────────────────► ──SessionClosed──► main（收口）
 *   注入： media WS 上行输入（{t:...}，显示器内归一化）──► 本模块 onInput（schema 校验 +
 *        monitor/virtualRect 换算，fix round 1 单跳换算）──raw_input──► 既有 CUA Helper broker
 *        （通道复用，不新起第二条 helper 生命周期；services 注入端口在被控开关打开时会经同一
 *        创建入口按需拉起）
 *
 * 纪律：
 * - host 不解析信令内容（P4 起 union 只剩 hello/monitors/select-monitor/media-open/freeze/
 *   unfreeze/bye/error），只按 shared `remoteDesktopSignalMessageSchema` 校验后转发；单帧 >64KiB 直接断开。
 * - 单会话互斥（第二请求 busy）、kill-switch/总开关在**发起侧**收口（evaluateRemoteDesktopRequest）。
 * - 被控状态持久化：总开关写凭据服务；逐设备授权写既有 lan client 记录上的可选字段
 *   （`remoteDesktopAuthorized`，旧数据缺键 = 未授权）。
 * - host 装配（index.ts）优先使用 T3b 的 services 注入端口 `injectCuaRawInput(events) → {dispatched}`；
 *   该端口经既有 helper 通道，不可达时报 helper_unavailable。未注入时（旧 services / 测试装配）
 *   回落到 `options.injectRawInput` 缺省实现：直连既有 broker socket，不拉起任何进程，
 *   socket 不可达时整帧失败并计数。
 */
import type { WebSocket } from "ws";
import {
  remoteDesktopControlEventSchema,
  remoteDesktopPointerEventSchema,
  remoteDesktopSignalMessageSchema,
  DEFAULT_REMOTE_DESKTOP_CODEC,
  DEFAULT_REMOTE_DESKTOP_QUALITY,
  REMOTE_DESKTOP_QUALITY_PRESETS,
  type RemoteDesktopCodec,
  type RemoteDesktopErrorCode,
  type RemoteDesktopQuality,
} from "@mode/shared/remote-desktop";
import {
  encoderCapabilityFromStartResult,
  loadPersistedEncoderCapability,
  persistEncoderCapability,
  type EncoderCapability,
} from "./encoderCapabilityStore.js";
import type { ICredentialService } from "@mode/services";
import { callBrokerMethod, resolveBrokerSocketPath } from "@mode/cua/broker";
import {
  REMOTE_DESKTOP_INJECT_MAX_EVENTS,
  REMOTE_DESKTOP_SIGNAL_MAX_FRAME_BYTES,
  RemoteDesktopHostMessageTypes,
  isRemoteDesktopMessageTypeTag,
  remoteDesktopMessageToHostSchema,
  type RemoteDesktopMessageFromHost,
  type RemoteDesktopMessageToHost,
  type RemoteDesktopRawInputEvent,
} from "./remoteDesktopMessages.js";
import {
  convertControlEvent,
  convertPointerEvent,
  createRemoteDesktopInjectBatcher,
  toRemoteDesktopPxRect,
  type RemoteDesktopPxRect,
} from "./inputConversion.js";
import {
  isRemoteDesktopKillSwitchEngaged,
  type LanAccessServerHandle,
} from "./lanAccessServer.js";
import {
  readLanAccessClientRecords,
  writeLanAccessClientRecords,
  type LanAccessClientRecordWithRemoteDesktop,
} from "./lanAccessRecords.js";
import { evaluateRemoteDesktopRequest } from "./remoteDesktopAdmission.js";
import { createRemoteDesktopMediaRelay } from "./remoteDesktopMediaRelay.js";

/** 被控总开关的凭据键（与 lan client 记录同居一个凭据存储；不做第二份配置）。 */
const REMOTE_DESKTOP_SERVER_ENABLED_CREDENTIAL_KEY = "remote-desktop:server-enabled";

/** 同意等待上限：产品规则 2 的 60s 超时由 main 的同意窗执行，这里是 main 失约时的兜底。 */
const CONSENT_SAFETY_TIMEOUT_MS = 75_000;
/** 同意通过后等待 WS 接线的上限：与票据 TTL(30s) 同量级，过期即释放单会话占用。 */
const PENDING_SOCKET_TTL_MS = 35_000;
const INJECT_TIMEOUT_MS = 5_000;

export interface RemoteDesktopHostLogger {
  info(...args: unknown[]): void;
  warn(...args: unknown[]): void;
}

/** 注入端口：events（VIRTUALDESK 0..65535 已换算）→ dispatched 条数。 */
export type RemoteDesktopInjectPort = (
  events: RemoteDesktopRawInputEvent[],
) => Promise<{ dispatched: number }>;

export interface RemoteDesktopHostServer {
  /** lanAccessServer 的 hooks（同意流 + 信令交接 + 媒体 WS 交接）。 */
  hooks: {
    requestSession(
      clientId: string,
      quality?: RemoteDesktopQuality,
      codec?: RemoteDesktopCodec,
    ): Promise<{ ok: true } | { error: RemoteDesktopErrorCode }>;
    /**
     * `session.reissueMediaTicket`：媒体 WS 瞬断重连（P2-1）——票据 store 在 server 层
     * （createHttpServer），宿主在此处理 `{type:"request-media-ticket"}` 时用它重签。
     */
    onSessionSocket(
      clientId: string,
      socket: WebSocket,
      session?: { reissueMediaTicket?: () => { mediaTicket: string; expiresAt: number } },
    ): void;
    onMediaSocket(clientId: string, socket: WebSocket): void;
  };
  /** 能力位读取点（同步缓存，握手时现取；开关变更后立刻生效）。 */
  isServerEnabled(): boolean;
  /**
   * 本机是否处于被控中：同意等待、待接线或进行中会话均算（设备页置灰口径）。
   * 与 Electron main 的 coordinator 状态机同义；Tauri 产品壳没有 main，由 Host 直接给事实。
   */
  isControlled(): boolean;
  /** 启动时把持久化的开关读入同步缓存（准备就绪前按「关」fail-closed）。 */
  loadServerState(): Promise<void>;
  /**
   * 被控会话状态变化（同意等待 / 已接管 / 已结束）。
   * 指示条据此显示/隐藏；只在状态真的变化时触发。
   */
  onDidChangeControlledState(listener: (controlled: boolean) => void): { dispose(): void };
  handleMessage(message: RemoteDesktopMessageToHost): void;
  dispose(): void;
}

/**
 * rd_media helper 注入端口：method ∈ rd_media_*（napi 面），params 为 helper 契约参数。
 * `rd_media_start` 的返回值被透传给调用方判编码器来源（`{encoder:"hardware"|"software"}`，
 * 见 `encoderCapabilityFromStartResult`）；switch/stop 的返回值不关心。
 */
export type RemoteDesktopMediaCall = (
  method: "rd_media_start" | "rd_media_switch" | "rd_media_stop",
  params: object,
) => Promise<unknown>;

interface PendingConsent {
  clientId: string;
  resolve: (result: { ok: true } | { error: RemoteDesktopErrorCode }) => void;
  timer: ReturnType<typeof setTimeout>;
}

/** 待接线会话（同意通过但 WS 尚未到达）；ticket 一次性消费的 clientId 绑定在此复核。 */
interface PendingSocket {
  clientId: string;
  timer: ReturnType<typeof setTimeout>;
}

export function createRemoteDesktopHostServer(options: {
  credentials: ICredentialService;
  logger: RemoteDesktopHostLogger;
  /** host → main 的消息出口（parentPort.postMessage 的薄包装）。 */
  postToMain(message: RemoteDesktopMessageFromHost): void;
  /** 运行期活着的 lanAccessServer：它才是 lan client 记录的内存写者。 */
  getActiveLanAccessServer: () => LanAccessServerHandle | null;
  /** 注入端口覆盖点（T3b / 测试）；缺省 = broker 直连兜底。 */
  injectRawInput?: RemoteDesktopInjectPort;
  /** rd_media helper 注入端口（Task 8 注入真实现）；未注入时 Media* 分支静默跳过，注入后失败仅 warn、不断会话。 */
  callRdMedia?: RemoteDesktopMediaCall;
  /**
   * 编码能力缓存持久化路径（HEVC/能力缓存 2026-10-13；缺省 = 不持久化，纯内存）。
   * 真实会话观测即写回；GPU 变更后的过期值由下一次真实会话自愈覆盖。
   */
  encoderCapabilityPath?: string;
}): RemoteDesktopHostServer {
  const { credentials, logger } = options;
  let serverEnabled = false;
  let pendingConsent: PendingConsent | null = null;
  let pendingSocket: PendingSocket | null = null;
  let activeSession: { clientId: string; socket: WebSocket } | null = null;
  /** 本会话画质档位（P1-3）：requestSession 时按主控选择写入，MediaStart 据此出参数。 */
  let sessionQuality: RemoteDesktopQuality = DEFAULT_REMOTE_DESKTOP_QUALITY;
  /**
   * 本会话编码 codec 偏好（HEVC 选型 2026-10-13）：requestSession 时按主控
   * `POST?codec=` 写入，MediaStart 参数直达 helper——helper 侧无硬件 HEVC 会回退
   * h264（warn），主控解码按码流自描述跟随，故此处只记偏好不校验能力。
   */
  let sessionCodec: RemoteDesktopCodec = DEFAULT_REMOTE_DESKTOP_CODEC;
  /**
   * 编码能力缓存（P1-3 → 2026-10-15 增 software 态）：从真实会话的 rd_media_start 结果
   * 观测（含 helper 回报的编码器来源），避免被控端为探测额外拉起一次 helper；会话发生前
   * 恒为 unknown（不猜测）。software = 本机无硬件编码器、已回退系统软件编码（能出画面，
   * 但 CPU/延迟更高）；unavailable = 硬软两路都拿不到编码器（会话根本起不来）。
   * 持久化（2026-10-13）：观测值按 `encoderCapabilityPath` 落盘、启动时回读
   * （冷启动即可显示提示）；不按 GPU 指纹分卡——GPU 变更由下一次真实会话自愈。
   */
  let encoderCapability: EncoderCapability = "unknown";
  /** 被控状态监听（指示条）：只在 `isControlled()` 真的变化时通知。 */
  const controlledListeners = new Set<(controlled: boolean) => void>();
  let lastControlled = false;
  const notifyControlled = (): void => {
    const controlled = pendingConsent !== null || pendingSocket !== null || activeSession !== null;
    if (controlled === lastControlled) {
      return;
    }
    lastControlled = controlled;
    for (const listener of controlledListeners) {
      try {
        listener(controlled);
      } catch {
        // 订阅方异常不得影响会话状态机。
      }
    }
  };
  /** 观测值统一入口：内存更新 + 异步落盘（写失败只 warn，不影响会话）。 */
  const setEncoderCapability = (next: EncoderCapability): void => {
    encoderCapability = next;
    const path = options.encoderCapabilityPath;
    if (path) {
      void persistEncoderCapability(path, next).catch((error: unknown) => {
        logger.warn("[remote-desktop] 编码能力缓存写入失败:", error);
      });
    }
  };
  if (options.encoderCapabilityPath) {
    // 启动回读（fire-and-forget）：首个真实会话观测必然晚于本读取；读取失败 = 无缓存。
    void loadPersistedEncoderCapability(options.encoderCapabilityPath).then((stored) => {
      if (stored) {
        encoderCapability = stored;
      }
    });
  }
  /**
   * 当前媒体连接（clientId → socket）。本表只负责登记与断开清理；
   * 收发路由（分帧透传、pong、输入）由 mediaRelay.attachWebSocket 接管。
   */
  const mediaSockets = new Map<string, WebSocket>();
  let injectDroppedFrames = 0;
  /** 媒体 WS 上行中被丢弃的输入事件数（畸形形状 / 换算基准未就绪）：计数用于联调观测。 */
  let mediaInputDroppedEvents = 0;
  /**
   * 输入换算基准（monitor 物理 rect + 虚拟屏 union rect）：MediaStart 建立、MediaSwitch 更新、
   * 会话收口清空。两者都由 coordinator 在发消息时现算（显示器变化后下一条消息自动带新值）。
   */
  let inputRects: { monitor: RemoteDesktopPxRect; virtual: RemoteDesktopPxRect } | null = null;

  /**
   * 媒体中继（本地管道 ↔ media WS 的字节泵）。本模块只负责实例化、attach 与会话收口三件事；
   * 回调接线：onStatus → MediaStatus 通知 main（冻结透传链路的 host 出口，T6）；
   * onReady 保持日志位——ready→开流指令（kind2）由 relay 内部幂等完成，接实体会双发 kind2。
   * onFrame 不注入：走 relay 缺省出口（当前 WS + 128KB 背压丢帧 + 丢帧即请关键帧，契约 C）。
   * onInput（T14）：media WS 上行输入 → 既有 raw_input 注入（见下）。
   */
  /**
   * 媒体面**起不来**（rd_media 注入端口缺失 / rd_media_start 失败）→ 通知 main 回主控
   * `error{unsupported}` 并收口。这是「采集从未建立」而非「采集中断」（后者走 frozen）：
   * 不报的话主控只能对着零帧干等（2026-10-14 实机：状态条「画面已连接 / 0 fps / 0.0 Mbps」）。
   * spec: lan-remote-desktop-native-media.md「媒体面可观测性与首帧约束」。
   */
  const reportMediaUnavailable = (unavailable: string): void =>
    options.postToMain({
      type: RemoteDesktopHostMessageTypes.RemoteDesktopMediaStatus,
      frozen: false,
      unavailable,
    });

  const mediaRelay = createRemoteDesktopMediaRelay({
    logger,
    onStatus: (frozen, reason) =>
      options.postToMain({
        type: RemoteDesktopHostMessageTypes.RemoteDesktopMediaStatus,
        frozen,
        ...(reason ? { reason } : {}),
      }),
    onReady: () => log("media ws ready received"),
    // 实体接线（Task 14 fix round 1：host 单跳换算）：media WS 上行是契约 C 的既有输入事件
    // （{t:...}，显示器内归一化 0..1，属不可信面）——先按 shared schema 严格校验，再用
    // MediaStart/MediaSwitch 携带的 monitor+virtualRect 换算为 {k:...} VIRTUALDESK 绝对事件，
    // 拆 ≤512 帧后交给既有注入端口（单会话门/帧上限/丢帧计数统一在 inject 内收口）。
    onInput: (event) => {
      const rects = inputRects;
      if (!rects) {
        // 开流基准未就绪（MediaStart 之前）：renderer 输入只在媒体面建立后到达，先丢并计数。
        dropMediaInput("no coordinate reference yet (before media start)");
        return;
      }
      const pointer = remoteDesktopPointerEventSchema.safeParse(event);
      if (pointer.success) {
        // 合批（spec 事件顺序 ④）：pointer 进 0ms 聚合窗口同 tick 合并，不再逐事件一条管道连接。
        injectBatcher.enqueuePointer(
          convertPointerEvent(pointer.data, rects.monitor, rects.virtual),
        );
        return;
      }
      const control = remoteDesktopControlEventSchema.safeParse(event);
      if (control.success) {
        // control 即时 flush（缓冲的 pointer 先出，保持同帧先后）。
        injectBatcher.enqueueControl(
          convertControlEvent(control.data, rects.monitor, rects.virtual),
        );
        return;
      }
      dropMediaInput("not a control/pointer event");
    },
  });
  let disposed = false;

  const log = (message: string, detail?: unknown): void =>
    detail === undefined
      ? logger.info(`[remote-desktop] ${message}`)
      : logger.info(`[remote-desktop] ${message}`, detail);

  // ---- 设置存储（凭据服务；与 lan client 记录同一存储机制）----

  const loadEnabled = async (): Promise<boolean> => {
    const raw = await credentials.load(REMOTE_DESKTOP_SERVER_ENABLED_CREDENTIAL_KEY);
    return raw === "1";
  };
  const persistEnabled = async (enabled: boolean): Promise<void> => {
    if (enabled) {
      await credentials.save(REMOTE_DESKTOP_SERVER_ENABLED_CREDENTIAL_KEY, "1");
    } else {
      await credentials.delete(REMOTE_DESKTOP_SERVER_ENABLED_CREDENTIAL_KEY);
    }
  };

  /** 读授权记录：有活 handle 时以它的内存数组为准（唯一写者），否则直接读凭据。 */
  const readClients = async (): Promise<LanAccessClientRecordWithRemoteDesktop[]> => {
    const live = options.getActiveLanAccessServer();
    if (live) {
      return live
        .getState()
        .clients.filter((client) => client.id)
        .map((client) => ({ ...client }));
    }
    return await readLanAccessClientRecords(credentials);
  };

  const setAuthorized = async (clientId: string, authorized: boolean): Promise<void> => {
    const live = options.getActiveLanAccessServer();
    if (live) {
      await live.setClientRemoteDesktopAuthorized(clientId, authorized);
      return;
    }
    // 无活服务端：本机没有第二个写者（lanAccessServer 起停由 host 串行管理），可安全读改写。
    const records = await readLanAccessClientRecords(credentials);
    const target = records.find((record) => record.id === clientId);
    if (!target) {
      return;
    }
    if (authorized) {
      target.remoteDesktopAuthorized = true;
    } else {
      delete target.remoteDesktopAuthorized;
    }
    await writeLanAccessClientRecords(credentials, records);
  };

  const authorizedClients = async (): Promise<Array<{ id: string; label: string }>> =>
    (await readClients())
      .filter((client) => client.remoteDesktopAuthorized === true)
      .map((client) => ({ id: client.id, label: client.label }));

  const postServerState = async (requestId: string): Promise<void> => {
    try {
      options.postToMain({
        type: RemoteDesktopHostMessageTypes.RemoteDesktopServerStateResult,
        requestId,
        ok: true,
        enabled: serverEnabled,
        authorizedClients: await authorizedClients(),
        encoderCapability,
      });
    } catch (error) {
      options.postToMain({
        type: RemoteDesktopHostMessageTypes.RemoteDesktopServerStateResult,
        requestId,
        ok: false,
        enabled: serverEnabled,
        authorizedClients: [],
        encoderCapability,
        error: error instanceof Error ? error.message : String(error),
      });
    }
  };

  // ---- 会话（单会话互斥 + 同意流）----

  const labelOf = async (clientId: string): Promise<string> => {
    const record = (await readClients()).find((client) => client.id === clientId);
    return record?.label?.trim() || "LAN client";
  };

  const settleConsent = (result: { ok: true } | { error: RemoteDesktopErrorCode }): void => {
    const pending = pendingConsent;
    if (!pending) {
      return;
    }
    pendingConsent = null;
    clearTimeout(pending.timer);
    if ("ok" in result) {
      // 同意通过 → 等该 clientId 的 WS 到达（票据 TTL 30s；过期未接线即释放 busy 占用，
      // 否则一次「同意后从不连」会把被控端永久卡在 busy）。
      pendingSocket = {
        clientId: pending.clientId,
        timer: setTimeout(() => {
          if (pendingSocket?.clientId === pending.clientId) {
            pendingSocket = null;
            logger.info(
              `[remote-desktop] pending session socket expired, clientId=${pending.clientId}`,
            );
          }
        }, PENDING_SOCKET_TTL_MS),
      };
      // 同意通过即开中继口（127.0.0.1 随机口；listen 幂等，同一会话重复触发复用同一端口）。
      // 端口只记日志：helper 侧 endpoint 由后续 MediaStart 流程按 listen 返回值取用。
      void mediaRelay
        .listen()
        .then((port) => log(`media relay listening on 127.0.0.1:${port}`))
        .catch((error: unknown) => {
          logger.warn(
            "[remote-desktop] media relay listen failed:",
            error instanceof Error ? error.message : error,
          );
        });
    }
    pending.resolve(result);
    notifyControlled();
  };

  const clearPendingSocket = (): void => {
    if (pendingSocket) {
      clearTimeout(pendingSocket.timer);
      pendingSocket = null;
    }
    notifyControlled();
  };

  const requestSession = async (
    clientId: string,
    quality?: RemoteDesktopQuality,
    codec?: RemoteDesktopCodec,
  ): Promise<{ ok: true } | { error: RemoteDesktopErrorCode }> => {
    if (disposed) {
      return { error: "disabled" };
    }
    const admission = evaluateRemoteDesktopRequest({
      killSwitchEngaged: isRemoteDesktopKillSwitchEngaged(process.env),
      serverEnabled,
    });
    if (admission.action === "reject") {
      return { error: admission.error };
    }
    if (pendingConsent || pendingSocket || activeSession) {
      // 单主控单被控（spec 非目标：多主控并发）：占用中直接 busy，不弹第二个同意窗。
      return { error: "busy" };
    }
    // 画质档位随请求写入（缺省 = 旧主控 → 默认档；未知值由 schema 在入口层拦下）。
    sessionQuality = quality ?? DEFAULT_REMOTE_DESKTOP_QUALITY;
    // 编码 codec 偏好随请求写入（缺省 = 旧主控 → h264）；helper 侧 HEVC 缺失自动回退。
    sessionCodec = codec ?? DEFAULT_REMOTE_DESKTOP_CODEC;
    const label = await labelOf(clientId);
    return await new Promise<{ ok: true } | { error: RemoteDesktopErrorCode }>((resolve) => {
      const timer = setTimeout(() => {
        // main 未在兜底窗口内回帖（正常应由同意窗 60s 超时先回）：fail-closed 收口。
        logger.warn(`[remote-desktop] consent result missing, clientId=${clientId}`);
        settleConsent({ error: "consent_timeout" });
      }, CONSENT_SAFETY_TIMEOUT_MS);
      pendingConsent = { clientId, resolve, timer };
      notifyControlled();
      log(`consent requested, clientId=${clientId}`);
      options.postToMain({
        type: RemoteDesktopHostMessageTypes.RemoteDesktopConsentRequest,
        clientId,
        label,
      });
    });
  };

  const closeActiveSocket = (code: number, reason: string): void => {
    // closeActiveSocket 只在会话终止路径被调用（peer/本地 bye、信号帧超限、关开关、dispose）。
    // 中继必须在这里也收口：本函数先把 activeSession 置空，随后 session socket 的 close 事件里
    // wasActive 判 false 会跳过 clearSessionState——只挂在那里，这四条路径会残留中继口/helper
    // 连接/媒体 WS（Task 7 落地后残留 helper 会在下一会话触发「第二条连接拒绝」堵死重连）。
    // close() 幂等，与 clearSessionState 中的调用重复无害；activeSession 为空时同样收口
    // （同意后、信令 WS 未接的 pending 窗口也已开中继，关开关等终止动作应一并释放）。
    mediaRelay.close();
    const session = activeSession;
    activeSession = null;
    notifyControlled();
    if (!session) {
      return;
    }
    try {
      session.socket.close(code, reason);
    } catch {
      // 远端已断开时 close 允许幂等失败。
    }
  };

  /** 结束收口唯一出口：清 pending/active 全量引用，socket 关闭由调用方决定是否触发。 */
  const clearSessionState = (): void => {
    if (pendingConsent) {
      settleConsent({ error: "internal" });
    }
    clearPendingSocket();
    // 会话收口：中继口、helper 连接与媒体 WS 一并释放（close 幂等；下一次会话 listen 重开随机口）。
    mediaRelay.close();
    // 输入合批缓冲随会话清空：残留的未 flush 输入不补发（避免结束后的输入落到下一会话）。
    injectBatcher.reset();
    // 输入换算基准随会话销毁：下一会话必须由新的 MediaStart 重新建立（不跨会话复用旧显示器几何）。
    inputRects = null;
  };

  const onSessionSocket = (
    clientId: string,
    socket: WebSocket,
    session?: { reissueMediaTicket?: () => { mediaTicket: string; expiresAt: number } },
  ): void => {
    if (disposed || !pendingSocket || pendingSocket.clientId !== clientId) {
      // 没有对应的同意通过记录（票据被别的已配对设备出示/会话已收口）：不留下无归属信令连接。
      try {
        socket.close(4001, "remote desktop session unavailable");
      } catch {
        // 幂等失败忽略。
      }
      return;
    }
    clearPendingSocket();
    activeSession = { clientId, socket };
    notifyControlled();
    log(`session socket attached, clientId=${clientId}`);

    socket.on("message", (data: unknown) => {
      const text =
        typeof data === "string" ? data : Buffer.isBuffer(data) ? data.toString("utf8") : "";
      if (!text) {
        return;
      }
      if (Buffer.byteLength(text, "utf8") > REMOTE_DESKTOP_SIGNAL_MAX_FRAME_BYTES) {
        logger.warn("[remote-desktop] signal frame over 64KiB, closing session");
        closeActiveSocket(4009, "signal frame too large");
        options.postToMain({
          type: RemoteDesktopHostMessageTypes.RemoteDesktopSessionClosed,
          reason: "error",
        });
        return;
      }
      let parsed: unknown;
      try {
        parsed = JSON.parse(text);
      } catch {
        logger.warn("[remote-desktop] dropping malformed signal frame");
        return;
      }
      const message = remoteDesktopSignalMessageSchema.safeParse(parsed);
      if (!message.success) {
        logger.warn("[remote-desktop] dropping invalid signal frame");
        return;
      }
      // 媒体 WS 瞬断重连（P2-1）：会话内本地处理，不落信令转发面——
      // 重签一张媒体票（绑定同 clientId）回给主控 renderer，编码/中继全程不停。
      if (message.data.type === "request-media-ticket") {
        const issued = session?.reissueMediaTicket?.();
        if (!issued) {
          logger.warn(`[remote-desktop] media ticket reissue unavailable, clientId=${clientId}`);
          return;
        }
        try {
          socket.send(JSON.stringify({ type: "media-ticket", mediaTicket: issued.mediaTicket }));
          log(`media ticket reissued, clientId=${clientId}`);
        } catch (error) {
          logger.warn(
            "[remote-desktop] media ticket send failed:",
            error instanceof Error ? error.message : error,
          );
        }
        return;
      }
      // 方向越权：media-ticket 只允许被控 → 主控，主控方向出现即协议破坏，丢弃不转发。
      if (message.data.type === "media-ticket") {
        logger.warn("[remote-desktop] unexpected media-ticket from controller, dropped");
        return;
      }
      options.postToMain({
        type: RemoteDesktopHostMessageTypes.RemoteDesktopSignalFromHost,
        message: message.data,
      });
      if (message.data.type === "bye") {
        // 对端 bye 即结束（spec 事件顺序 ⑥ 任一端断开）：先转发给 main，再优雅关闭本端。
        closeActiveSocket(1000, "bye");
        options.postToMain({
          type: RemoteDesktopHostMessageTypes.RemoteDesktopSessionClosed,
          reason: "peer-end",
        });
        return;
      }
      if (message.data.type === "error") {
        // 主控端以 error 收口（例如协议不支持）：转发后关闭本端，不留半开会话。
        // Tauri 产品壳没有 main coordinator，这一步必须在 server 层收口。
        closeActiveSocket(4008, "peer error");
        options.postToMain({
          type: RemoteDesktopHostMessageTypes.RemoteDesktopSessionClosed,
          reason: "error",
        });
      }
    });

    socket.on("close", () => {
      const wasActive = activeSession?.clientId === clientId;
      if (wasActive) {
        activeSession = null;
        clearSessionState();
        notifyControlled();
        // 会话生命周期写 info 级日志（spec 安全边界：不记录画面内容）。
        log(`session closed (socket closed), clientId=${clientId}`);
        options.postToMain({
          type: RemoteDesktopHostMessageTypes.RemoteDesktopSessionClosed,
          reason: "socket-closed",
        });
      }
    });
    socket.on("error", (error: Error) => {
      logger.warn(`[remote-desktop] session socket error: ${error.message}`);
    });
  };

  /**
   * 媒体 WS 交接：归属复核 → 登记 mediaSockets → relay.attachWebSocket
   * （分帧透传、pong、输入路由都在 relay 内部的 message 订阅里完成，本函数不收发帧）；
   * 断开时按引用清理本表，relay 的 currentWs 由其自身的 close 订阅清理。
   * 同 clientId 重复升级（重连/旧票据迟到）时新连接顶替旧连接，避免双连接并存。
   */
  const onMediaSocket = (clientId: string, socket: WebSocket): void => {
    if (disposed) {
      try {
        socket.close(4001, "remote desktop disposed");
      } catch {
        // 幂等失败忽略。
      }
      return;
    }
    // 归属复核（复用 pendingSocket/activeSession 判定）：媒体 WS 只接受「同意通过待接线」
    // 或「进行中」会话的 clientId——票据是必要非充分条件，会话收口后的迟到票据不留连接。
    if (pendingSocket?.clientId !== clientId && activeSession?.clientId !== clientId) {
      logger.warn(
        `[remote-desktop] media socket without session owner rejected, clientId=${clientId}`,
      );
      try {
        socket.close(4001, "remote desktop session unavailable");
      } catch {
        // 幂等失败忽略。
      }
      return;
    }
    const previous = mediaSockets.get(clientId);
    if (previous && previous !== socket) {
      try {
        previous.close(4001, "replaced by newer media socket");
      } catch {
        // 幂等失败忽略。
      }
    }
    mediaSockets.set(clientId, socket);
    // relay 挂接（内部订阅 message/close/error；旧 currentWs 由 relay 以 1000 收口）。
    mediaRelay.attachWebSocket(socket);
    log(`media socket attached, clientId=${clientId}`);

    socket.on("close", () => {
      // 按引用清理：旧连接的迟到 close 事件不能删掉已顶替它的新连接。
      if (mediaSockets.get(clientId) === socket) {
        mediaSockets.delete(clientId);
        log(`media socket closed, clientId=${clientId}`);
      }
    });
    socket.on("error", (error: Error) => {
      logger.warn(`[remote-desktop] media socket error: ${error.message}`);
    });
  };

  // ---- 注入端口 ----

  const defaultInject: RemoteDesktopInjectPort = async (events) => {
    const socketPath = resolveBrokerSocketPath();
    const result = await callBrokerMethod<{ dispatched?: unknown }>({
      socketPath,
      method: "raw_input",
      params: { events },
      timeoutMs: INJECT_TIMEOUT_MS,
    });
    return {
      dispatched: typeof result?.dispatched === "number" ? result.dispatched : events.length,
    };
  };

  /**
   * 注入串行链（2026-10-13 修复，spec 事件顺序 ④）：绝对坐标事件跨帧乱序会表现为
   * 光标回跳——每帧注入（独立 broker 管道连接）必须等上一帧完成后再发起，
   * 同一时刻至多一帧在途。失败只计数不断链（catch 后链继续）。
   */
  let injectChain: Promise<void> = Promise.resolve();
  const inject = (events: RemoteDesktopRawInputEvent[]): void => {
    if (!activeSession) {
      return;
    }
    // 上限双保险：main 侧按帧已截断，这里再截一次保证任何来源都不越界（与 Rust 同口径）。
    const bounded =
      events.length > REMOTE_DESKTOP_INJECT_MAX_EVENTS
        ? events.slice(0, REMOTE_DESKTOP_INJECT_MAX_EVENTS)
        : events;
    if (bounded.length < events.length) {
      logger.warn(`[remote-desktop] inject frame truncated: ${events.length} → ${bounded.length}`);
    }
    const send = options.injectRawInput ?? defaultInject;
    injectChain = injectChain
      .then(async () => {
        // 返回值（dispatched 计数）不参与链：链只表达「上一帧注入完成后才发起下一帧」。
        await send(bounded);
      })
      .catch((error: unknown) => {
        injectDroppedFrames += 1;
        logger.warn(
          `[remote-desktop] inject dropped (${bounded.length} events, total frames dropped=${injectDroppedFrames}):`,
          error instanceof Error ? error.message : error,
        );
      });
  };

  /**
   * 输入合批器（spec 事件顺序 ④）：pointer 进 0ms 聚合窗口同 tick 合并（≤512 立即 flush
   * 兜底），control 即时 flush 且先出缓冲的 pointer 保持同帧先后；帧经上面的串行链注入。
   */
  const injectBatcher = createRemoteDesktopInjectBatcher({
    now: Date.now,
    schedule: (callback, delayMs) => setTimeout(callback, delayMs),
    cancel: (timer) => clearTimeout(timer),
    onFrame: (frame) => inject(frame),
  });

  /** 上行输入丢弃（畸形 / 换算基准未就绪）：计数 + 首个 warn（60fps 输入面不逐条刷屏）。 */
  const dropMediaInput = (reason: string): void => {
    mediaInputDroppedEvents += 1;
    if (mediaInputDroppedEvents === 1) {
      logger.warn(`[remote-desktop] media ws input dropped: ${reason}`);
    }
  };

  return {
    hooks: { requestSession, onSessionSocket, onMediaSocket },
    isServerEnabled: () => serverEnabled,
    isControlled: () => pendingConsent !== null || pendingSocket !== null || activeSession !== null,
    onDidChangeControlledState(listener) {
      controlledListeners.add(listener);
      return {
        dispose: () => {
          controlledListeners.delete(listener);
        },
      };
    },
    /** 启动即把持久化的被控开关读入同步缓存（能力位按每次握手现取，此处只做一次冷读）。 */
    loadServerState: async (): Promise<void> => {
      serverEnabled = await loadEnabled();
    },
    handleMessage(message: RemoteDesktopMessageToHost): void {
      if (disposed) {
        return;
      }
      if (message.type === RemoteDesktopHostMessageTypes.RemoteDesktopConsentResult) {
        const pending = pendingConsent;
        if (!pending || pending.clientId !== message.clientId) {
          return;
        }
        if (message.decision === "allow") {
          if (message.remember) {
            void setAuthorized(message.clientId, true).catch((error: unknown) => {
              logger.warn(
                "[remote-desktop] persist device authorization failed:",
                error instanceof Error ? error.message : error,
              );
            });
          }
          log(`consent allowed, clientId=${message.clientId}, remember=${message.remember}`);
          settleConsent({ ok: true });
          return;
        }
        log(`consent denied, clientId=${message.clientId}, reason=${message.reason ?? "user"}`);
        settleConsent({
          error: message.reason === "timeout" ? "consent_timeout" : "consent_denied",
        });
        return;
      }
      if (message.type === RemoteDesktopHostMessageTypes.RemoteDesktopSignalToHost) {
        const session = activeSession;
        if (!session) {
          return;
        }
        const frame = JSON.stringify(message.message);
        if (Buffer.byteLength(frame, "utf8") > REMOTE_DESKTOP_SIGNAL_MAX_FRAME_BYTES) {
          logger.warn("[remote-desktop] outbound signal frame over 64KiB, dropping frame");
          return;
        }
        try {
          session.socket.send(frame);
        } catch (error) {
          logger.warn(
            "[remote-desktop] signal send failed:",
            error instanceof Error ? error.message : error,
          );
        }
        if (message.message.type === "bye") {
          // main 侧本地结束（指示窗「结束」/应用退出）：bye 已带出，随后关闭本端 socket。
          closeActiveSocket(1000, "bye");
        }
        return;
      }
      if (message.type === RemoteDesktopHostMessageTypes.RemoteDesktopMediaStart) {
        // 输入换算基准随开流建立（同步生效，先于 helper 起流）：monitor 物理 rect + 虚拟屏
        // union rect，后者由 coordinator 发消息时现算（显示器变化后下一条消息自动带新值）。
        inputRects = {
          monitor: toRemoteDesktopPxRect(message.rect),
          virtual: toRemoteDesktopPxRect(message.virtualRect),
        };
        // 开流：同意时已 listen 过中继口（幂等，同会话复用同一随机口），这里取端口注入
        // rd_media_start 给 helper。P1-3：启动参数取本会话画质档位（fps/maxWidth/初始
        // 码率），替代旧硬编码 1920/60/8Mbps；自适应码率由主控在档位区间内调节。
        // callRdMedia 由 Task 8 注入。原生媒体面是画面唯一来源（纯直替 WebRTC 后 P2 阶段
        // 的「画面仍在」前提已不成立）：未注入或启动失败都不得静默——主控侧只会表现为
        // 「画面已连接、fps 0、码率 0.0Mbps」的纯黑，双端零日志则无法定位
        //（spec「媒体面可观测性与首帧约束」2026-10-14）。会话仍不断：主控靠首帧看门狗提示。
        void (async () => {
          const callRdMedia = options.callRdMedia;
          if (!callRdMedia) {
            logger.warn(
              "[remote-desktop] media start skipped: rd_media helper port not injected（本机起了会话但不会出画面）",
            );
            reportMediaUnavailable("rd_media-helper-port-not-injected");
            return;
          }
          try {
            const port = await mediaRelay.listen();
            const preset = REMOTE_DESKTOP_QUALITY_PRESETS[sessionQuality];
            const result = await callRdMedia("rd_media_start", {
              rect: message.rect,
              maxWidth: preset.maxWidth,
              fps: preset.fps,
              bitrateBps: preset.bitrateBps,
              // codec 偏好直达 helper（HEVC 选型 2026-10-13）：无硬件 HEVC 由 helper 回退。
              codec: sessionCodec,
              endpoint: { host: "127.0.0.1", port },
              key: message.key,
              salt: message.salt,
            });
            // 编码能力缓存（P1-3 → 2026-10-15 增 software 态）：真实会话成功即按 helper 回报的
            // 编码器来源记 available/software（无需额外探测拉起 helper）；setEncoderCapability
            // 同步落盘，冷启动即可显示。
            setEncoderCapability(encoderCapabilityFromStartResult(result));
          } catch (error) {
            const messageText = error instanceof Error ? error.message : String(error);
            // 硬编与软件回退两路都拿不到编码器是「会话根本起不来」的根因：单独记住，设置页给提示。
            // 匹配的是 Rust 侧两路合成后的错误串（`无可用 H.264 编码器（硬件口径：…；软件口径：…）`）。
            if (messageText.includes("无可用 H.264 编码器")) {
              setEncoderCapability("unavailable");
            }
            logger.warn("[remote-desktop] rd_media_start failed:", messageText);
            // 同上：起不来必须回稳定错误码，不能让主控对着零帧等（码面用既有 unsupported）。
            reportMediaUnavailable(`rd_media-start-failed:${messageText}`);
          }
        })();
        return;
      }
      if (message.type === RemoteDesktopHostMessageTypes.RemoteDesktopMediaSwitch) {
        // 换屏同时刷新输入换算基准（monitor 换成新显示器、virtual 用本轮现算值）。
        inputRects = {
          monitor: toRemoteDesktopPxRect(message.rect),
          virtual: toRemoteDesktopPxRect(message.virtualRect),
        };
        void options
          .callRdMedia?.("rd_media_switch", { rect: message.rect })
          .catch((error: unknown) => {
            logger.warn(
              "[remote-desktop] rd_media_switch failed:",
              error instanceof Error ? error.message : error,
            );
          });
        return;
      }
      if (message.type === RemoteDesktopHostMessageTypes.RemoteDesktopMediaStop) {
        // relay.close 不在此做：会话收口（closeActiveSocket / clearSessionState）已覆盖，
        // 这里双关只会把下一次会话的幂等 listen 语义搅浑。
        void options.callRdMedia?.("rd_media_stop", {}).catch((error: unknown) => {
          logger.warn(
            "[remote-desktop] rd_media_stop failed:",
            error instanceof Error ? error.message : error,
          );
        });
        return;
      }
      if (message.type === RemoteDesktopHostMessageTypes.RemoteDesktopServerStateRequest) {
        void postServerState(message.requestId);
        return;
      }
      if (message.type === RemoteDesktopHostMessageTypes.RemoteDesktopSetServerEnabled) {
        const next = message.enabled;
        void (async () => {
          try {
            await persistEnabled(next);
            serverEnabled = next;
            log(`server enabled set to ${String(next)}`);
            if (!next) {
              // 关开关 = 本地结束（spec 事件顺序 ⑥）：终止进行中的会话。
              closeActiveSocket(1000, "server disabled");
            }
          } catch (error) {
            logger.warn(
              "[remote-desktop] persist server enabled failed:",
              error instanceof Error ? error.message : error,
            );
          }
          await postServerState(message.requestId);
        })();
        return;
      }
      if (message.type === RemoteDesktopHostMessageTypes.RemoteDesktopRevokeClient) {
        void (async () => {
          await setAuthorized(message.clientId, false).catch((error: unknown) => {
            logger.warn(
              "[remote-desktop] revoke client failed:",
              error instanceof Error ? error.message : error,
            );
          });
          await postServerState(message.requestId);
        })();
        return;
      }
      if (message.type === RemoteDesktopHostMessageTypes.RemoteDesktopAuthorizeClient) {
        // 设置页授权开关：与同意窗「记住此设备」写同一记录字段（同一 setAuthorized 收口）。
        void (async () => {
          await setAuthorized(message.clientId, true).catch((error: unknown) => {
            logger.warn(
              "[remote-desktop] authorize client failed:",
              error instanceof Error ? error.message : error,
            );
          });
          await postServerState(message.requestId);
        })();
      }
    },
    dispose(): void {
      if (disposed) {
        return;
      }
      disposed = true;
      // 应用退出/窗口关闭：先给主控带出 bye，再关 WS（spec 事件顺序 ⑥ 的本地发起端）。
      const active = activeSession;
      if (active) {
        try {
          active.socket.send(JSON.stringify({ type: "bye", reason: "remote-end" }));
        } catch {
          // 通道已坏时直接进入收尾。
        }
      }
      clearSessionState();
      closeActiveSocket(1000, "disposed");
      // 媒体连接随 server 收口：不留半开的媒体 WS（票据已消费，连接本身也要释放）。
      for (const socket of mediaSockets.values()) {
        try {
          socket.close(1000, "disposed");
        } catch {
          // 幂等失败忽略。
        }
      }
      mediaSockets.clear();
      controlledListeners.clear();
    },
  };
}

/** 解析 host 侧入口消息：类型标签不在 RD 集合内 → null（调用方继续走共享 schema）。 */
export function parseRemoteDesktopMessageToHost(value: unknown): RemoteDesktopMessageToHost | null {
  const type = (value as { type?: unknown } | null)?.type;
  if (typeof type !== "string" || !isRemoteDesktopMessageTypeTag(type)) {
    return null;
  }
  const parsed = remoteDesktopMessageToHostSchema.safeParse(value);
  return parsed.success ? parsed.data : null;
}
