/* eslint-disable max-lines -- 被控 host 侧会话/设置/注入集中维护同一套单会话互斥与收口纪律，拆散会让收口路径分散。 */
/**
 * 远程桌面**被控 host 侧**协调（docs/specs/lan-remote-desktop.md 事件顺序 ②③⑤⑥）。
 *
 * 端口方向图（本模块是 host 侧唯一入口）：
 *
 *   lanAccessServer ──(hook)requestSession(clientId)──────► 本模块 ──ConsentRequest──► 被控 main
 *   WS /ws/remote-desktop?ticket ◄──(hook)onSessionSocket── 本模块 ◄─ConsentResult─── 被控 main
 *        │ 收到的每一帧 ──────────────────────────────────► ──SignalFromHost──► main（隐藏采集窗）
 *        │ ◄──────────────────────────────────────────────── ◄─SignalToHost──── main
 *        └─ 断开/bye ─────────────────────────────────────► ──SessionClosed──► main（收口）
 *   注入： main ──Inject(≤512/帧)──► 本模块 ──raw_input──► 既有 CUA Helper broker（通道复用，
 *        不新起第二条 helper 生命周期；services 注入端口在被控开关打开时会经同一创建入口按需拉起）
 *
 * 纪律：
 * - host 不解析 SDP/ICE，只按 shared `remoteDesktopSignalMessageSchema` 校验后转发；单帧 >64KiB 直接断开。
 * - 单会话互斥（第二请求 busy）、kill-switch/总开关在**发起侧**收口（evaluateRemoteDesktopRequest）。
 * - 被控状态持久化：总开关写凭据服务；逐设备授权写既有 lan client 记录上的可选字段
 *   （`remoteDesktopAuthorized`，旧数据缺键 = 未授权）。
 * - host 装配（index.ts）优先使用 T3b 的 services 注入端口 `injectCuaRawInput(events) → {dispatched}`；
 *   该端口经既有 helper 通道，不可达时报 helper_unavailable。未注入时（旧 services / 测试装配）
 *   回落到 `options.injectRawInput` 缺省实现：直连既有 broker socket，不拉起任何进程，
 *   socket 不可达时整帧失败并计数。
 */
import type { WebSocket } from "ws";
import { remoteDesktopSignalMessageSchema, type RemoteDesktopErrorCode } from "@mode/shared/remote-desktop";
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
  isRemoteDesktopKillSwitchEngaged,
  readLanAccessClientRecords,
  writeLanAccessClientRecords,
  type LanAccessClientRecordWithRemoteDesktop,
  type LanAccessServerHandle,
} from "./lanAccessServer.js";
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
    requestSession(clientId: string): Promise<{ ok: true } | { error: RemoteDesktopErrorCode }>;
    onSessionSocket(clientId: string, socket: WebSocket): void;
    onMediaSocket(clientId: string, socket: WebSocket): void;
  };
  /** 能力位读取点（同步缓存，握手时现取；开关变更后立刻生效）。 */
  isServerEnabled(): boolean;
  /** 启动时把持久化的开关读入同步缓存（准备就绪前按「关」fail-closed）。 */
  loadServerState(): Promise<void>;
  handleMessage(message: RemoteDesktopMessageToHost): void;
  dispose(): void;
}

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
}): RemoteDesktopHostServer {
  const { credentials, logger } = options;
  let serverEnabled = false;
  let pendingConsent: PendingConsent | null = null;
  let pendingSocket: PendingSocket | null = null;
  let activeSession: { clientId: string; socket: WebSocket } | null = null;
  /**
   * 当前媒体连接（clientId → socket）。本表只负责登记与断开清理；
   * 收发路由（分帧透传、pong、输入）由 mediaRelay.attachWebSocket 接管。
   */
  const mediaSockets = new Map<string, WebSocket>();
  /**
   * 媒体中继（本地管道 ↔ media WS 的字节泵）。本模块只负责实例化、attach 与会话收口三件事；
   * 回调实体接线归属：T6 接 onStatus/onReady、T14 接 onInput/onFrame——当前只放日志位，
   * 勿在此提前接实体。kind0 出口不注入 onFrame：走 relay 缺省实现（当前 WS + 4MB 背压丢弃，
   * 契约 C），即「出口机制在 relay 内、消费实体留后续任务」。
   */
  const mediaRelay = createRemoteDesktopMediaRelay({
    logger,
    onStatus: (frozen, reason) =>
      log(`media pipe status frozen=${frozen}${reason === undefined ? "" : `, reason=${reason}`}`),
    onReady: () => log("media ws ready received"),
    onInput: () => logger.warn("[remote-desktop] media input not wired yet (T14)"),
  });
  let injectDroppedFrames = 0;
  let disposed = false;

  const log = (message: string, detail?: unknown): void =>
    detail === undefined ? logger.info(`[remote-desktop] ${message}`) : logger.info(`[remote-desktop] ${message}`, detail);

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
      });
    } catch (error) {
      options.postToMain({
        type: RemoteDesktopHostMessageTypes.RemoteDesktopServerStateResult,
        requestId,
        ok: false,
        enabled: serverEnabled,
        authorizedClients: [],
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
            logger.info(`[remote-desktop] pending session socket expired, clientId=${pending.clientId}`);
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
  };

  const clearPendingSocket = (): void => {
    if (pendingSocket) {
      clearTimeout(pendingSocket.timer);
      pendingSocket = null;
    }
  };

  const requestSession = async (
    clientId: string,
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
    const label = await labelOf(clientId);
    return await new Promise<{ ok: true } | { error: RemoteDesktopErrorCode }>((resolve) => {
      const timer = setTimeout(() => {
        // main 未在兜底窗口内回帖（正常应由同意窗 60s 超时先回）：fail-closed 收口。
        logger.warn(`[remote-desktop] consent result missing, clientId=${clientId}`);
        settleConsent({ error: "consent_timeout" });
      }, CONSENT_SAFETY_TIMEOUT_MS);
      pendingConsent = { clientId, resolve, timer };
      log(`consent requested, clientId=${clientId}`);
      options.postToMain({
        type: RemoteDesktopHostMessageTypes.RemoteDesktopConsentRequest,
        clientId,
        label,
      });
    });
  };

  const closeActiveSocket = (code: number, reason: string): void => {
    const session = activeSession;
    activeSession = null;
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
  };

  const onSessionSocket = (clientId: string, socket: WebSocket): void => {
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
    log(`session socket attached, clientId=${clientId}`);

    socket.on("message", (data: unknown) => {
      const text = typeof data === "string" ? data : Buffer.isBuffer(data) ? data.toString("utf8") : "";
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
      }
    });

    socket.on("close", () => {
      const wasActive = activeSession?.clientId === clientId;
      if (wasActive) {
        activeSession = null;
        clearSessionState();
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
      logger.warn(`[remote-desktop] media socket without session owner rejected, clientId=${clientId}`);
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

  const inject = (events: RemoteDesktopRawInputEvent[]): void => {
    if (!activeSession) {
      return;
    }
    // 上限双保险：main 侧按帧已截断，这里再截一次保证任何来源都不越界（与 Rust 同口径）。
    const bounded = events.length > REMOTE_DESKTOP_INJECT_MAX_EVENTS ? events.slice(0, REMOTE_DESKTOP_INJECT_MAX_EVENTS) : events;
    if (bounded.length < events.length) {
      logger.warn(`[remote-desktop] inject frame truncated: ${events.length} → ${bounded.length}`);
    }
    const send = options.injectRawInput ?? defaultInject;
    void send(bounded).catch((error: unknown) => {
      injectDroppedFrames += 1;
      logger.warn(
        `[remote-desktop] inject dropped (${bounded.length} events, total frames dropped=${injectDroppedFrames}):`,
        error instanceof Error ? error.message : error,
      );
    });
  };

  return {
    hooks: { requestSession, onSessionSocket, onMediaSocket },
    isServerEnabled: () => serverEnabled,
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
        log(
          `consent denied, clientId=${message.clientId}, reason=${message.reason ?? "user"}`,
        );
        settleConsent({ error: message.reason === "timeout" ? "consent_timeout" : "consent_denied" });
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
      if (message.type === RemoteDesktopHostMessageTypes.RemoteDesktopInject) {
        inject(message.events);
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
      }
    },
    dispose(): void {
      if (disposed) {
        return;
      }
      disposed = true;
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
    },
  };
}

/** 解析 host 侧入口消息：类型标签不在 RD 集合内 → null（调用方继续走共享 schema）。 */
export function parseRemoteDesktopMessageToHost(
  value: unknown,
): RemoteDesktopMessageToHost | null {
  const type = (value as { type?: unknown } | null)?.type;
  if (typeof type !== "string" || !isRemoteDesktopMessageTypeTag(type)) {
    return null;
  }
  const parsed = remoteDesktopMessageToHostSchema.safeParse(value);
  return parsed.success ? parsed.data : null;
}
