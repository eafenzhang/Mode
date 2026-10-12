/**
 * 被控 host 侧**媒体中继**：本地管道（helper TCP ↔ host）与 media WS 之间的字节泵
 * （docs/specs/lan-remote-desktop-native-media.md 协议节：契约 B 管道、契约 C WS 消息）。
 *
 * 职责边界（本模块只做字节泵，不做业务）：
 * - 分帧：`[u32 len 大端][u8 kind][payload]` 的粘包/半包重组，超长帧防御后按 kind 分发；
 * - 路由：kind0 帧密文 → 出口回调（缺省 = 写当前 media WS，4MB 背压丢弃）；
 *   kind1 采集状态 → 只回调（冻结状态唯一所有者在 remoteDesktopServer，此处不落状态）；
 *   media WS 上行：ready → 开流指令（幂等一次）、ping → 立即 pong、其余对象 → 输入透传；
 * - 中继**不解密**：kind0 payload 原样搬运，明文密钥与明文帧都不经过 host（spec 安全节）。
 *
 * 回调实体接线归属：onStatus/onReady 由 Task 6 接（host↔main 消息）、onInput/onFrame 由
 * Task 14 接；本模块只保证注入点与缺省出口可用。
 */
import { createServer, type Server, type Socket } from "node:net";

/**
 * media WS 最小鸭子类型：只要求中继真正用到的四个成员，
 * 测试用 fake 对象即可实现（不引 `ws` 包类型，避免本模块与具体 WS 实现耦合）。
 */
export interface WsLike {
  send(data: string | Buffer, opts?: unknown): void;
  on?(event: string, cb: (...a: never[]) => void): void;
  close?(code?: number, reason?: string): void;
  bufferedAmount?: number;
}

export interface RemoteDesktopMediaRelayLogger {
  info(...args: unknown[]): void;
  warn(...args: unknown[]): void;
}

export interface RemoteDesktopMediaRelayOptions {
  logger: RemoteDesktopMediaRelayLogger;
  /**
   * kind0 出口；缺省实现 = 当前 WS 存在且 bufferedAmount ≤ 4MB 时 `ws.send(Buffer)`，
   * 否则丢弃（契约 C：断开即丢帧、背压只丢 kind0）。覆盖点留 Task 14。
   */
  onFrame?: (payload: Uint8Array) => void;
  /** kind1 采集状态（T6 接 → host↔main MediaStatus）。 */
  onStatus?: (frozen: boolean, reason?: string) => void;
  /** WS 首条 ready（T6 接；开流指令由本模块在 ready 时下发）。 */
  onReady?: () => void;
  /** WS 上行输入事件透传（T14 接 → broker raw_input）。 */
  onInput?: (event: unknown) => void;
}

export interface RemoteDesktopMediaRelay {
  /** 127.0.0.1 随机口中继口（幂等：已监听则复用同一端口），返回 port。 */
  listen(): Promise<number>;
  /** media WS 挂接（新连接顶替旧连接，旧的 close(1000,"replaced")）。 */
  attachWebSocket(ws: WsLike): void;
  /** helper TCP 数据入口（内部处理粘包/半包）。 */
  handlePipeData(chunk: Buffer): void;
  /** 向 helper 下发开流指令（[u32BE 1][0x02]）。 */
  sendToHelper(kind: 2): void;
  /** media WS 上行文本分发：ready / ping / 输入事件。 */
  handleWsMessage(data: string | Buffer, isBinary: boolean): void;
  /** 收口：关中继口 + helper 连接 + 当前 WS(1000)；幂等，可再次 listen 开新会话。 */
  close(): void;
}

/** 管道单帧上限：kind0 只承载逐帧密文，16MB 远超任何合法帧；超限即畸形流（长度已失真），立即断开。 */
const MAX_PIPE_FRAME_BYTES = 16 * 1024 * 1024;
/** media WS 背压线（契约 C）：bufferedAmount 超过 4MB 后丢弃后续 kind0，ping/输入不丢。 */
const WS_BACKPRESSURE_BYTES = 4 * 1024 * 1024;

export function createRemoteDesktopMediaRelay(
  options: RemoteDesktopMediaRelayOptions,
): RemoteDesktopMediaRelay {
  const { logger } = options;

  let server: Server | null = null;
  let listenPort: number | null = null;
  let helperSocket: Socket | null = null;
  /** 攒包缓冲：只在「已有半包」时 concat（新分配），已消费的前缀不会被原地改写，交给出口的视图始终稳定。 */
  let accum: Buffer = Buffer.alloc(0);
  let currentWs: WsLike | null = null;
  /** ready 幂等位：一条会话只下发一次开流指令；close 收口时复位（下一条会话可重新开流）。 */
  let readySent = false;
  /**
   * 开流指令已请求但 helper 管道尚未接通：接通即补发。
   * renderer 的 ready 与 helper 连入是并行的（同意→拉 helper 与主控解码就绪互不等待），
   * 先到的 ready 若直接丢弃开流指令会导致 helper 永远不推流。
   */
  let openRequested = false;
  /** 背压丢帧只 warn 一次（每个 WS 挂接周期），避免 60fps 下刷屏。 */
  let backpressureWarned = false;

  /** kind0 缺省出口（契约 C）：无 WS 即丢、过 4MB 背压即丢，其余原样转发给当前 media WS。 */
  const defaultFrameSink = (payload: Uint8Array): void => {
    const ws = currentWs;
    if (!ws) {
      return; // 断开即丢帧：中继不在无 WS 时缓冲，避免内存堆积与陈旧帧回灌。
    }
    if ((ws.bufferedAmount ?? 0) > WS_BACKPRESSURE_BYTES) {
      if (!backpressureWarned) {
        backpressureWarned = true;
        logger.warn("[rd-media-relay] media ws backpressure >4MB, dropping kind0 frames");
      }
      return;
    }
    try {
      ws.send(Buffer.from(payload));
    } catch (error) {
      logger.warn(
        "[rd-media-relay] media ws send failed:",
        error instanceof Error ? error.message : error,
      );
    }
  };
  const emitFrame = options.onFrame ?? defaultFrameSink;

  /** kind1 状态事件：只回调不落状态（冻结状态唯一所有者在 remoteDesktopServer）。 */
  const dispatchStatus = (payload: Buffer): void => {
    let parsed: unknown;
    try {
      parsed = JSON.parse(payload.toString("utf8"));
    } catch {
      logger.warn("[rd-media-relay] malformed kind1 status json, dropped");
      return;
    }
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
      logger.warn("[rd-media-relay] kind1 status is not an object, dropped");
      return;
    }
    const status = parsed as { frozen?: unknown; reason?: unknown };
    if (typeof status.frozen !== "boolean") {
      logger.warn("[rd-media-relay] kind1 status missing boolean frozen, dropped");
      return;
    }
    const reason = typeof status.reason === "string" ? status.reason : undefined;
    try {
      options.onStatus?.(status.frozen, reason);
    } catch (error) {
      logger.warn(
        "[rd-media-relay] onStatus callback failed:",
        error instanceof Error ? error.message : error,
      );
    }
  };

  /** 按 kind 分发完整管道帧体（body[0] = kind；len≥1 已在分帧处保证）。 */
  const dispatchPipeBody = (body: Buffer): void => {
    const kind = body[0];
    if (kind === 0) {
      // payload = [u32 seq][u8 flags][ciphertext‖tag]，原样交给出口（中继不解密）。
      try {
        emitFrame(body.subarray(1));
      } catch (error) {
        logger.warn(
          "[rd-media-relay] frame sink failed:",
          error instanceof Error ? error.message : error,
        );
      }
      return;
    }
    if (kind === 1) {
      dispatchStatus(body.subarray(1));
      return;
    }
    if (kind === 2) {
      // 方向越权：开流指令只允许 host→helper 下发，helper 侧出现即协议破坏。
      logger.warn("[rd-media-relay] unexpected kind=2 from helper, dropped");
      return;
    }
    logger.warn(`[rd-media-relay] unknown pipe kind=${String(kind)}, dropped`);
  };

  const destroyHelper = (reason: string): void => {
    const socket = helperSocket;
    helperSocket = null;
    accum = Buffer.alloc(0);
    if (socket) {
      logger.warn(`[rd-media-relay] closing helper pipe: ${reason}`);
      socket.destroy();
    }
  };

  const handlePipeData = (chunk: Buffer): void => {
    accum = accum.length === 0 ? chunk : Buffer.concat([accum, chunk]);
    while (accum.length >= 4) {
      const len = accum.readUInt32BE(0);
      if (len === 0 || len > MAX_PIPE_FRAME_BYTES) {
        // 长度字段已失真（len 至少要装下 1 字节 kind）：继续读只会越读越歪，直接断开。
        destroyHelper(`invalid frame length ${len}`);
        return;
      }
      if (accum.length < 4 + len) {
        return; // 半包：等 helper 的后续 chunk 补齐再分帧。
      }
      const body = accum.subarray(4, 4 + len);
      accum = accum.subarray(4 + len);
      dispatchPipeBody(body);
    }
  };

  const sendToHelper = (kind: 2): void => {
    const socket = helperSocket;
    if (!socket || socket.destroyed) {
      logger.warn(`[rd-media-relay] send kind=${kind} skipped: helper not connected`);
      return;
    }
    const frame = Buffer.alloc(5);
    frame.writeUInt32BE(1, 0); // len = 1（仅 kind 字节，无 payload）
    frame.writeUInt8(kind, 4);
    socket.write(frame);
  };

  /** 开流指令补发：ready 已请求且 helper 已接通时恰好下发一次。 */
  const flushOpenStream = (): void => {
    if (!openRequested) {
      return;
    }
    const socket = helperSocket;
    if (!socket || socket.destroyed) {
      return; // helper 仍未接通：保持挂起，等它的 connection 事件再补发。
    }
    openRequested = false;
    sendToHelper(2);
  };

  const handleWsMessage = (data: string | Buffer, isBinary: boolean): void => {
    if (isBinary) {
      // 契约 C 上行只有文本 JSON；二进制上行无定义，丢弃不猜。
      logger.warn("[rd-media-relay] binary upstream message not in contract, dropped");
      return;
    }
    const text = typeof data === "string" ? data : data.toString("utf8");
    let parsed: unknown;
    try {
      parsed = JSON.parse(text);
    } catch {
      logger.warn("[rd-media-relay] malformed upstream json, dropped");
      return;
    }
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
      logger.warn("[rd-media-relay] upstream message is not an object, dropped");
      return;
    }
    const message = parsed as { t?: unknown; ts?: unknown };
    if (message.t === "ready") {
      if (readySent) {
        return; // 幂等：重复 ready 不重发开流指令（helper 侧只应被叫醒一次）。
      }
      readySent = true;
      try {
        options.onReady?.();
      } catch (error) {
        logger.warn(
          "[rd-media-relay] onReady callback failed:",
          error instanceof Error ? error.message : error,
        );
      }
      openRequested = true;
      if (!helperSocket || helperSocket.destroyed) {
        logger.info("[rd-media-relay] open-stream request pending: helper pipe not connected yet");
      }
      flushOpenStream();
      return;
    }
    if (message.t === "ping") {
      // RTT 应答不经背压队列（契约 C：ping/输入不丢），也不经 helper。
      try {
        currentWs?.send(JSON.stringify({ t: "pong", ts: message.ts }));
      } catch (error) {
        logger.warn(
          "[rd-media-relay] pong send failed:",
          error instanceof Error ? error.message : error,
        );
      }
      return;
    }
    try {
      options.onInput?.(parsed);
    } catch (error) {
      logger.warn(
        "[rd-media-relay] onInput callback failed:",
        error instanceof Error ? error.message : error,
      );
    }
  };

  /** listen() 在途共享 promise：首调 await 期间（server 尚未赋值）第二调短路，防建出第二个 server 泄漏。 */
  let listeningPromise: Promise<number> | null = null;

  const startListening = async (): Promise<number> => {
    const srv = createServer((socket) => {
      if (server !== srv) {
        // close() 收口后才排队到达的 connection 事件：直接拒绝，
        // 否则这条连接无人收口（helperSocket 被 close 置空后又被回填），残留到下一会话。
        socket.destroy();
        return;
      }
      if (helperSocket && !helperSocket.destroyed) {
        // 单会话单 helper：第二条连接（重复 rd_media_start/异常重启）直接拒绝。
        logger.warn("[rd-media-relay] second helper connection rejected");
        socket.destroy();
        return;
      }
      helperSocket = socket;
      accum = Buffer.alloc(0); // 新连接从零攒包，避免上一条连接残留的半包污染。
      socket.on("data", (chunk: Buffer) => {
        try {
          handlePipeData(chunk);
        } catch (error) {
          logger.warn(
            "[rd-media-relay] pipe data handling failed:",
            error instanceof Error ? error.message : error,
          );
        }
      });
      socket.on("error", (error: Error) => {
        logger.warn(`[rd-media-relay] helper pipe error: ${error.message}`);
        if (helperSocket === socket) {
          helperSocket = null;
          accum = Buffer.alloc(0);
        }
      });
      socket.on("close", () => {
        if (helperSocket === socket) {
          helperSocket = null;
          accum = Buffer.alloc(0);
          logger.info("[rd-media-relay] helper pipe closed");
        }
      });
      logger.info("[rd-media-relay] helper pipe connected");
      flushOpenStream(); // ready 先于 helper 到达时，在此补发被挂起的开流指令。
    });
    await new Promise<void>((resolve, reject) => {
      const onListening = (): void => {
        srv.off("error", onError);
        resolve();
      };
      const onError = (error: Error): void => {
        srv.off("listening", onListening);
        reject(error);
      };
      srv.once("listening", onListening);
      srv.once("error", onError);
      // 只绑 127.0.0.1：中继口是同机管道，绝不对外暴露（spec 状态所有者表）。
      srv.listen({ port: 0, host: "127.0.0.1" });
    }).catch((error: unknown) => {
      server = null;
      listenPort = null;
      throw error;
    });
    const address = srv.address();
    if (address === null || typeof address === "string") {
      srv.close();
      server = null;
      listenPort = null;
      throw new Error("[rd-media-relay] listen succeeded without an inet port");
    }
    srv.on("error", (error: Error) => {
      logger.warn(`[rd-media-relay] relay server error: ${error.message}`);
    });
    server = srv;
    listenPort = address.port;
    return listenPort;
  };

  const listen = (): Promise<number> => {
    if (server && listenPort !== null) {
      return Promise.resolve(listenPort); // 幂等：同一会话重复触发（同意 + MediaStart）复用同一随机口。
    }
    if (listeningPromise) {
      return listeningPromise; // 并发防重入：在途调用共享同一 promise，不建第二个 server。
    }
    listeningPromise = startListening().finally(() => {
      listeningPromise = null;
    });
    return listeningPromise;
  };

  const attachWebSocket = (ws: WsLike): void => {
    if (currentWs === ws) {
      // 同一 socket 重复挂接（调用方重复触发）直接短路：否则 message/close 监听叠加，
      // 一条 ping 会被派发两次（双 pong）、close 清理也执行两遍。
      return;
    }
    const previous = currentWs;
    if (previous && previous !== ws) {
      try {
        previous.close?.(1000, "replaced"); // 旧连接收口（1000 = 正常关闭语义）。
      } catch {
        // 远端可能已断：close 幂等失败忽略。
      }
    }
    currentWs = ws;
    backpressureWarned = false; // 新连接重新给一次背压 warn 观测机会。
    ws.on?.("message", (data: unknown, isBinary: unknown) => {
      if (currentWs !== ws) {
        return; // 被顶替的旧连接迟到消息：不驱动中继。
      }
      if (typeof data === "string" || Buffer.isBuffer(data)) {
        handleWsMessage(data, isBinary === true);
        return;
      }
      logger.warn("[rd-media-relay] unsupported upstream message shape, dropped");
    });
    ws.on?.("close", () => {
      if (currentWs === ws) {
        currentWs = null; // 断开即丢帧的兑现点：此后 kind0 走缺省出口直接丢弃。
      }
    });
    ws.on?.("error", (error: unknown) => {
      logger.warn(
        `[rd-media-relay] media ws error: ${error instanceof Error ? error.message : String(error)}`,
      );
    });
    logger.info("[rd-media-relay] media ws attached");
  };

  const close = (): void => {
    const srv = server;
    server = null;
    listenPort = null;
    if (srv?.listening) {
      try {
        srv.close(); // 停止接受新连接；已 accept 的连接在下面逐个收口。
      } catch {
        // 幂等失败忽略（可能已被外部关闭）。
      }
    }
    const socket = helperSocket;
    helperSocket = null;
    accum = Buffer.alloc(0);
    if (socket) {
      try {
        socket.destroy();
      } catch {
        // 幂等失败忽略。
      }
    }
    const ws = currentWs;
    currentWs = null;
    if (ws) {
      try {
        ws.close?.(1000, "session closed");
      } catch {
        // 幂等失败忽略。
      }
    }
    readySent = false; // 会话收口：下一条会话重新允许下发开流指令。
    openRequested = false;
    backpressureWarned = false;
  };

  return { listen, attachWebSocket, handlePipeData, sendToHelper, handleWsMessage, close };
}
