/* oxlint-disable eslint(max-lines) -- 媒体面 socket/解码/重连/统计共享同一会话生命周期，拆散会分散收口路径。 */
/**
 * 原生媒体面 renderer 会话（spec: docs/specs/lan-remote-desktop-native-media.md）。
 * 纯函数（帧头/IV/SPS）不碰 DOM；胶水部分用 WebCodecs + WS（仅桌面 Chromium）。
 *
 * 下行帧布局（契约 B v2，helper 生产、这里消费，逐字节一致）：
 *   [u32 seq 大端][u8 flags(bit0=关键帧)][u32 capturedAtMs（被控时钟，帧龄起点）][ciphertext ‖ GCM tag(16)]
 * IV 不随帧传：iv = salt(8B) ∥ seq(4B 大端)，aes-256-gcm（spec 安全节）。
 */
import type { RemoteDesktopMediaSocket } from "@mode/shared/remote-desktop";

import { logger } from "@/logger.js";
import { nextReconnectDelayMs, RECONNECT_MAX_ATTEMPTS } from "@/remote-desktop/reconnectPolicy.js";
import { codecStringFromAnnexB } from "./codecString.js";
import { createFirstFrameWatchdog } from "./firstFrameWatchdog.js";
import { MEDIA_SOCKET_OPEN, openMediaSocket } from "./mediaSocket.js";

// codec 串派生与 HEVC 解码预检拆至 codecString.ts（max-lines 约束）；
// re-export 保持既有消费方（会话胶水/测试）导入路径不变。
export { codecStringFromAnnexB, probeHevcDecodeSupport } from "./codecString.js";

export interface ParsedMediaFrame {
  seq: number;
  key: boolean;
  /** 采集时刻（被控时钟 Unix 毫秒 u32 截断；0 = 未知）。帧龄按 32 位回绕差 + 时钟偏移计算。 */
  capturedAtMs: number;
  body: Uint8Array;
}

/** 切帧头：不足 9 字节（残包）返回 null，由调用方丢弃。 */
export function parseFrameHeader(bytes: Uint8Array): ParsedMediaFrame | null {
  if (bytes.length < 9) return null;
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  return {
    seq: view.getUint32(0, false),
    key: (bytes[4]! & 1) === 1,
    capturedAtMs: view.getUint32(5, false),
    body: bytes.subarray(9),
  };
}

/** 重建帧 IV：salt 前 8 字节 + seq 大端（与 helper 加密侧同一派生口径）。 */
export function buildFrameIv(salt: Uint8Array, seq: number): Uint8Array {
  const iv = new Uint8Array(12);
  iv.set(salt.subarray(0, 8), 0);
  new DataView(iv.buffer).setUint32(8, seq >>> 0, false);
  return iv;
}

/**
 * 解码队列门控（纯函数，测试见 remoteDesktopNativeSession.test.ts）：
 * delta 帧在解码排队超过 `MAX_DECODE_QUEUE` 时丢弃（继续排队只会累计端到端延迟）；
 * 关键帧永远放行——它是重同步点。
 *
 * 2026-11-13（spec: remote-desktop-latency §2.2/§2.4）：门控从「>2 就丢、下一次采样
 * 又继续解」改成「进入重同步，只等关键帧」。原口径丢了 delta 仍继续解后续 delta，
 * 参考帧链已断、画面带残影，而且 300/500ms 的 need-key 去抖会把 IDR 刷成风暴。
 * 阈值也放宽到 4：硬解正常波动不应该触发整段重同步。
 */
const MAX_DECODE_QUEUE = 4;
/** 待处理的已到达帧上限（含正在解密的一帧）：超过即丢最旧 delta 并重同步。 */
const MAX_QUEUED_FRAMES = 2;
/** 关键帧请求去抖：一次重同步只请求一枚，避免 IDR 反噬链路（spec §2.4）。 */
const KEY_REQUEST_COOLDOWN_MS = 2_000;

export function shouldDropDeltaFrame(isKey: boolean, decodeQueueSize: number): boolean {
  return !isKey && decodeQueueSize > MAX_DECODE_QUEUE;
}

/** 采集戳是 u32 截断的 Unix 毫秒；真实帧龄 ≪ 2^32 ms，按 32 位回绕取差值。 */
const CAPTURED_AT_MODULUS = 2 ** 32;
/** 回绕后仍超过该值说明时钟偏移失准（或负帧龄回绕），不进入统计避免巨大读数。 */
const MAX_PLAUSIBLE_FRAME_AGE_MS = 60_000;

/**
 * 帧龄（glass-to-glass 近似，纯函数供测试）：
 * `now + 时钟偏移` 还原被控时钟的「现在」，再与 u32 截断的采集戳做 32 位回绕差。
 * 偏移标定正确时截断不引入误差（真帧龄 < 2^32 ms）；偏移失准或负帧龄回绕会得到
 * 巨大值，返回 null 让调用方跳过本帧读数。
 */
export function computeFrameAgeMs(
  nowMs: number,
  capturedAtMs: number,
  clockOffsetMs: number,
): number | null {
  const wrapped =
    (((nowMs + clockOffsetMs - capturedAtMs) % CAPTURED_AT_MODULUS) + CAPTURED_AT_MODULUS) %
    CAPTURED_AT_MODULUS;
  return wrapped <= MAX_PLAUSIBLE_FRAME_AGE_MS ? wrapped : null;
}

export interface NativeMediaSessionOptions {
  url: string;
  keyHex: string;
  saltHex: string;
  canvas: HTMLCanvasElement;
  /**
   * socket 工厂（缺省 `new WebSocket(url)`）。桌面端直连本机回环代理；
   * 浏览器端注入隧道适配器，把媒体地址映射到 E2EE 数据面。
   */
  openSocket?: (url: string) => RemoteDesktopMediaSocket;
  onConfigured?: (info: { width: number; height: number; codec: string }) => void;
  onDecoded?: () => void;
  /**
   * 首个已解码帧到达（每个 media WS 生命周期一次）：页面据此撤除「等待对端首帧」提示。
   */
  onFirstFrame?: () => void;
  /**
   * 首帧看门狗触发（每个 media WS 生命周期至多一次）：open 后窗口内零帧——被控未出流。
   * 页面据此切可见提示，不再以无提示纯黑呈现（spec「媒体面可观测性与首帧约束」）。
   */
  onNoFirstFrame?: (info: { waitedMs: number; bytesReceived: number }) => void;
  /**
   * 解码器建立/解码失败（HEVC 选型降级入口）：消费方可把 codec 偏好降为 H.264 并重建会话
   * （见 codecFallback.ts）。缺省 = 不降级，按既有的媒体失败路径收口。
   */
  onDecoderFailed?: (info: { message: string }) => void;
  /** 解码队列门控丢弃一帧时回调（自适应码率的「链路跟不上」信号源之一）。 */
  onFrameDropped?: () => void;
  /** 每绘制一帧回调其帧龄（毫秒，glass-to-glass 近似；需 pong 标定过时钟偏移）。 */
  onFrameAgeMs?: (ageMs: number) => void;
  /** 每条下行二进制帧回调其字节数（状态条近一拍平均码率的数据源）。 */
  onBytesReceived?: (bytes: number) => void;
  /** 每个被接受帧解码前回调其解码队列长度（状态条即时读数）。 */
  onDecodeQueue?: (size: number) => void;
  /**
   * 媒体 WS 瞬断重连（P2-1）：请求一张新媒体票并返回新的媒体地址（信令通道存活时可用；
   * 失败/未接线返回 null）。返回 null 且退避未耗尽时按退避重试，耗尽后按永久失败收口。
   */
  onRequestMediaUrl?: () => Promise<string | null>;
  /** 重连状态变化：true = 进入退避（画面中断提示）；false = 重连成功（画面恢复）。 */
  onReconnecting?: (reconnecting: boolean) => void;
  onTransportClosed?: () => void;
}

export interface NativeMediaSession {
  /** ready 由 ws open 发出（首条 send 亦作幂等补发保险）；随后原样上行事件对象。 */
  send(event: object): void;
  /**
   * 上行通道是否就绪（媒体 WS 处于 OPEN）。
   * 为什么需要它：`send` 在未 OPEN（会话尚未 open-media / 正在重连）时是**静默 no-op**，
   * 对「允许丢」的输入透传没问题，但对「送出去才算数」的上行（剪贴板 `{t:"clip"}`：门一旦登记
   * 该值就再也不会判定为变化）必须先问通道，否则登记成功而发送落空 = 这次同步永久丢失。
   */
  isOpen(): boolean;
  ping(ts: number): void;
  /** 时钟偏移标定（被控时钟 − 本端时钟 ≈ hostTs − ts − RTT/2；pong 每拍更新）。 */
  setClockOffset(offsetMs: number): void;
  stop(): void;
}

export function createNativeMediaSession(opts: NativeMediaSessionOptions): NativeMediaSession {
  const key = hexToBytes(opts.keyHex, 32, "key");
  const salt = hexToBytes(opts.saltHex, 8, "salt");
  // desynchronized：跳过合成器同步（低延迟画布模式），直画路径少一层呈现延迟。
  const ctx = opts.canvas.getContext("2d", { alpha: false, desynchronized: true });
  if (!ctx) throw new Error("2d canvas unavailable");
  let decoder: VideoDecoder | null = null;
  let configuredCodec: string | null = null;
  let readySent = false;
  let stopped = false;
  // 编码器不给真实时间戳：EncodedVideoChunk 只要求时间戳单调，按 60fps 名义步长递增即可
  // （该值不参与绘制与统计；fps = 实际绘制帧率，由 onDecoded 侧统计）。
  let timestampBase = 0;
  // 热路径状态：AES key 会话级导入一次（原实现逐帧 importKey）；seq 提交水位（防乱序/迟到）；
  // need-key 去抖时间戳。
  const cryptoKeyPromise = crypto.subtle.importKey("raw", asBufferSource(key), "AES-GCM", false, [
    "decrypt",
  ]);
  let lastSubmittedSeq = -1;
  let lastKeyRequestAt = 0;
  // 有界帧队列（spec: remote-desktop-latency §2.3/§2.4）：并发 WebCrypto decrypt 的完成
  // 顺序无保证，必须串行处理；但旧实现用无界 promise 链，主线程一卡就把「已到达帧」
  // 全排进内存。这里改成「处理 1 帧 + 最多排 2 帧」，溢出时丢最旧 delta 并进入重同步，
  // 用一次 IDR 换回低延迟，而不是让延迟随会话时长单调上涨。
  const frameQueue: ParsedMediaFrame[] = [];
  let processing = false;
  let resyncing = false;
  // 帧龄：提交时按 chunk 时间戳记采集时刻，绘制时领回算出端到端年龄；
  // 时钟偏移由 pong 标定（未标定前只跳过帧龄读数，画面不受影响）。
  const capturedAtByTimestamp = new Map<number, number>();
  let clockOffsetMs: number | null = null;
  // 媒体 WS 瞬断重连（P2-1）状态：当前 socket、退避计数/定时器、重连后的重同步标志
  //（断流期间中继丢帧 → 只放行关键帧并强制重配解码器，防花屏残留）。
  let ws: RemoteDesktopMediaSocket | null = null;
  let reconnectAttempts = 0;
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  let awaitingKeyAfterReconnect = false;
  let forceReconfigureNextKey = false;
  // 可观测性（spec「媒体面可观测性与首帧约束」）：会话级计数 + 每 socket 一次的首帧看门狗。
  // 「连上但一帧不出」必须在日志与状态条上自证——2026-10-14 实证：黑屏时本机日志零行。
  let bytesReceived = 0;
  let decodedFrames = 0;
  let wsOpenCount = 0;
  /** 本 socket open 时刻：首帧到达时记一次「open→首帧」耗时（起流快慢的可观测读数）。 */
  let socketOpenedAt = 0;
  /** 日志用端点：只 host:port——mediaTicket 不进日志（安全节）。 */
  const endpointLabel = (() => {
    try {
      return new URL(opts.url).host;
    } catch {
      return "unknown";
    }
  })();

  // 首帧看门狗（spec「媒体面可观测性与首帧约束」）：定时与判定收在 firstFrameWatchdog，
  // 这里只接三个落点——open 起表、画帧停表、收口清表；「零帧」的 warn 与计数在此落盘。
  const firstFrameWatchdog = createFirstFrameWatchdog({
    bytesReceived: () => bytesReceived,
    isStopped: () => stopped,
    onNoFrames: ({ waitedMs, bytesReceived: bytes }) => {
      logger.warn(
        `[rd-media] 媒体面已连接 ${Math.round(waitedMs / 1000)}s 零帧` +
          `（收流 ${bytes} 字节，ws=${endpointLabel}）：对端未出流`,
      );
      opts.onNoFirstFrame?.({ waitedMs, bytesReceived: bytes });
    },
  });

  /** 紧急关键帧请求（重同步/重连）：ws 上行 {t:"need-key"}；2s 去抖
   *  （中继侧另有 1.5s 去抖）；force = 重连路径绕过去抖（断流后的第一诉求）。 */
  const requestKey = (force = false): void => {
    const now = Date.now();
    if (!force && now - lastKeyRequestAt < KEY_REQUEST_COOLDOWN_MS) {
      return;
    }
    lastKeyRequestAt = now;
    if (ws && ws.readyState === MEDIA_SOCKET_OPEN) {
      ws.send(JSON.stringify({ t: "need-key" }));
    }
  };

  /** 退避耗尽/无重连能力：按永久失败收口（与旧行为一致）。 */
  const scheduleReconnect = (): void => {
    reconnectAttempts += 1;
    if (reconnectAttempts > RECONNECT_MAX_ATTEMPTS || !opts.onRequestMediaUrl) {
      logger.warn(
        `[rd-media] 重连退避耗尽/不可用，媒体面收口（尝试 ${reconnectAttempts - 1}/${RECONNECT_MAX_ATTEMPTS} 次，` +
          `收流 ${bytesReceived} 字节，已解码 ${decodedFrames} 帧）`,
      );
      teardown();
      return;
    }
    logger.info(
      `[rd-media] 重连尝试 ${reconnectAttempts}/${RECONNECT_MAX_ATTEMPTS}（${endpointLabel}）`,
    );
    opts.onReconnecting?.(true);
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null;
      void attemptReconnect();
    }, nextReconnectDelayMs(reconnectAttempts));
  };

  /** 一轮重连尝试：要新票 → 新媒体地址 → 重建 socket；拿不到票也走退避（退避计数递增）。 */
  const attemptReconnect = async (): Promise<void> => {
    if (stopped) {
      return;
    }
    let url: string | null = null;
    try {
      url = (await opts.onRequestMediaUrl?.()) ?? null;
    } catch {
      url = null;
    }
    if (stopped) {
      return;
    }
    if (!url) {
      scheduleReconnect();
      return;
    }
    attachSocket(openMediaSocket(opts.openSocket, url), true);
  };

  /**
   * socket 挂接（初连/重连共用）：所有 handler 都做 `ws === socket` 身份核对，
   * 被顶替/收口后的迟到事件不驱动状态。close 非本端收口 → 进入退避重连。
   */
  const attachSocket = (socket: RemoteDesktopMediaSocket, isReconnect: boolean): void => {
    ws = socket;
    socket.binaryType = "arraybuffer";
    socket.onopen = () => {
      if (ws !== socket || stopped) {
        return;
      }
      wsOpenCount += 1;
      socketOpenedAt = Date.now();
      logger.info(
        `[rd-media] ws open（第 ${wsOpenCount} 次，ws=${endpointLabel}${isReconnect ? "，重连" : ""}）`,
      );
      if (isReconnect) {
        // 重连成功：重置退避；断流期间丢帧使 decoder 状态过时——只放行关键帧，
        // 首个关键帧强制重配解码器（防花屏残留）；立刻请关键帧缩短重同步时间。
        reconnectAttempts = 0;
        awaitingKeyAfterReconnect = true;
        forceReconfigureNextKey = true;
        opts.onReconnecting?.(false);
      }
      if (!readySent) {
        // 幂等补发保险（初连）：ready 必须恰好一次。重连不需要重发——中继的 ready 门控
        // 是每条会话一次，重复 ready 会被中继幂等忽略。
        socket.send(JSON.stringify({ t: "ready" }));
        readySent = true;
      }
      // 开流即请 IDR（spec「媒体面可观测性与首帧约束」，初连与重连同）：被控在收到 ready
      // 前丢弃全部采集帧，静态桌面下捕获无事件 → 首个 IDR 可能永不产生；ready 之后立刻
      // 上行一次 need-key（绕过去抖）把「开流到出画」收紧为一次编码往返。
      requestKey(true);
      firstFrameWatchdog.arm();
    };
    socket.onmessage = (event) => {
      if (ws !== socket || stopped) {
        return;
      }
      handleMessage(event);
    };
    socket.onclose = () => {
      if (ws !== socket) {
        return;
      }
      ws = null;
      // 收口/重连共用的收尾读数：一帧没画就断的会话必须能从这里看出（零帧 ≠ 低帧率）。
      logger.info(
        `[rd-media] ws closed（第 ${wsOpenCount} 次连接，收流 ${bytesReceived} 字节，已解码 ${decodedFrames} 帧）`,
      );
      firstFrameWatchdog.clear();
      if (stopped) {
        return;
      }
      scheduleReconnect();
    };
    socket.onerror = () => {
      // onclose 必然跟随：收口/重连只在 onclose 决策一次，避免双路径。
    };
  };

  const configure = (codec: string) => {
    decoder?.close();
    // 重配即作废旧时间戳→采集时刻映射（旧码流的解码产物不再与新会话对齐）。
    capturedAtByTimestamp.clear();
    decoder = new VideoDecoder({
      output: (frame) => {
        // 只保留最新帧语义：解码即画（LAN 硬解 60fps 下无排队积压；帧率计数交给 onDecoded）。
        if (
          opts.canvas.width !== frame.displayWidth ||
          opts.canvas.height !== frame.displayHeight
        ) {
          opts.canvas.width = frame.displayWidth;
          opts.canvas.height = frame.displayHeight;
          // 首帧或 SPS 重配后的画面尺寸变化：外层据此更新坐标映射与状态条分辨率。
          opts.onConfigured?.({ width: frame.displayWidth, height: frame.displayHeight, codec });
        }
        ctx.drawImage(frame, 0, 0);
        const capturedAtMs = capturedAtByTimestamp.get(frame.timestamp);
        capturedAtByTimestamp.delete(frame.timestamp);
        frame.close();
        decodedFrames += 1;
        if (firstFrameWatchdog.noteFrame()) {
          // 本 socket 首个已画帧：撤除「等待对端首帧」提示（每 socket 只通知一次），
          // 并留一行起流读数——「连上到出画」的耗时是排障时最常问的一个数。
          logger.info(
            `[rd-media] 首帧已到（open 后 ${Date.now() - socketOpenedAt}ms，ws=${endpointLabel}）`,
          );
          opts.onFirstFrame?.();
        }
        opts.onDecoded?.();
        // 帧龄 = 本端绘制时刻 − 被控采集时刻 + 时钟偏移（glass-to-glass 近似）；
        // 采集戳是 u32 截断值，按 32 位回绕取差值；缺采集戳（旧端/未知=0）或
        // 未标定偏移/偏移失准 → 跳过本帧读数。
        if (typeof capturedAtMs === "number" && capturedAtMs !== 0 && clockOffsetMs !== null) {
          const ageMs = computeFrameAgeMs(Date.now(), capturedAtMs, clockOffsetMs);
          if (ageMs !== null) opts.onFrameAgeMs?.(ageMs);
        }
      },
      error: (e) => {
        logger.warn("[rd-media] decoder error:", e);
        // 降级入口先于收口：消费方可借此把 HEVC 偏好降为 H.264 并重建会话
        //（codecFallback）；不降级时 teardown 走既有失败归因。
        opts.onDecoderFailed?.({ message: e instanceof Error ? e.message : String(e) });
        teardown();
      },
    });
    try {
      decoder.configure({ codec, optimizeForLatency: true });
    } catch (error) {
      // codec 串不被浏览器接受（HEVC 派生串异常/平台不支持）：按媒体面失败收口，
      // 不留黑屏死等（teardown → onTransportClosed → 会话失败归因）。
      logger.warn("[rd-media] decoder configure failed:", error);
      opts.onDecoderFailed?.({
        message: error instanceof Error ? error.message : String(error),
      });
      try {
        decoder.close();
      } catch {
        // 关闭失败不掩盖主错误。
      }
      decoder = null;
      configuredCodec = null;
      teardown();
      return;
    }
    configuredCodec = codec;
  };
  /** 进入重同步：丢掉队列里的非关键帧，只等下一个 IDR；请求按 2s 去抖。 */
  const enterResync = (): void => {
    if (resyncing) return;
    resyncing = true;
    for (let index = frameQueue.length - 1; index >= 0; index -= 1) {
      if (!frameQueue[index]!.key) {
        frameQueue.splice(index, 1);
        opts.onFrameDropped?.();
      }
    }
    requestKey();
  };

  /** 入队并排空：溢出时丢最旧 delta 并重同步，队列长度恒 ≤ MAX_QUEUED_FRAMES+1。 */
  const enqueueFrame = (frame: ParsedMediaFrame): void => {
    // 关键帧是重同步点：它之前的 delta 参考链已缺，直接丢掉，保证关键帧先被处理。
    // 不在入队时丢「重同步期间的 delta」——关键帧还在解密时紧跟的 delta 可能已经
    // 以关键帧为参考（真机 60fps 下很常见），交给有界队列 + 处理顺序即可。
    if (frame.key && resyncing) {
      for (let index = frameQueue.length - 1; index >= 0; index -= 1) {
        if (!frameQueue[index]!.key) {
          frameQueue.splice(index, 1);
          opts.onFrameDropped?.();
        }
      }
    }
    frameQueue.push(frame);
    if (frameQueue.length > MAX_QUEUED_FRAMES) {
      const dropIndex = frameQueue.findIndex((item) => !item.key);
      frameQueue.splice(dropIndex >= 0 ? dropIndex : 0, 1);
      opts.onFrameDropped?.();
      enterResync();
    }
    void drainFrames();
  };

  /** 串行处理队列（解密完成顺序无保证，必须按到达顺序逐帧走）。 */
  const drainFrames = async (): Promise<void> => {
    if (processing) return;
    processing = true;
    try {
      while (frameQueue.length > 0 && !stopped) {
        const frame = frameQueue.shift()!;
        await processFrame(frame);
      }
    } finally {
      processing = false;
    }
  };

  // ws 一开就发 ready：helper 的 ready 门控只认第一条开流指令，而被控状态机保证首帧必为 IDR
  // （先 IDR 后 delta），所以不必等任何上行事件——否则画面要等用户首次移动鼠标/按键才出流，开局全黑。
  // （ready 发送在 attachSocket 的 onopen 里，初连与重连共用同一幂等位。）
  const handleMessage = (event: { data: unknown }): void => {
    if (typeof event.data === "string") {
      let msg: { t?: string };
      try {
        msg = JSON.parse(event.data) as { t?: string };
      } catch {
        // 下行 JSON 是协议内数据：解析失败按单条丢弃，不让坏数据变成未捕获异常。
        return;
      }
      if (
        msg.t === "pong" ||
        msg.t === "freeze" ||
        msg.t === "unfreeze" ||
        // 剪贴板文本（P1.1）：与 freeze/unfreeze 同一订阅面（rd-media-control），
        // 由会话面的剪贴板同步模块消费——应用与开关都在那里，本模块只转发。
        msg.t === "clip"
      ) {
        // freeze/unfreeze/pong/clip 通道：Task 10/13 订阅（替代已删除的 reducer 信号直染）。
        window.dispatchEvent(new CustomEvent("rd-media-control", { detail: msg }));
      }
      return;
    }
    const parsed = parseFrameHeader(new Uint8Array(event.data as ArrayBuffer));
    // 收口后不再处理任何帧：decoder error 与 ws error 路径不关 socket（只等消费方 stop），
    // 缺了 stopped 判据，收口后的关键帧会重新建 decoder 复活解码。
    if (!parsed || stopped) return;
    // 码率读数：按收到的密文字节数计（含帧头与 GCM tag，≈ 实际占线带宽）。
    const receivedBytes = (event.data as ArrayBuffer).byteLength;
    bytesReceived += receivedBytes;
    opts.onBytesReceived?.(receivedBytes);
    // 防乱序/迟到：按 seq 水位过滤已提交的帧（廉价前置，省一次解密）。
    if (parsed.seq <= lastSubmittedSeq) return;
    // 有界队列 + 串行处理：见 frameQueue 注释（spec: remote-desktop-latency §2.3）。
    enqueueFrame(parsed);
  };

  /** 单帧后续处理：解密 → 关键帧建/换 decoder → 队列门控 → 提交解码。 */
  const processFrame = async (parsed: ParsedMediaFrame): Promise<void> => {
    if (stopped) return;
    let plain: Uint8Array;
    try {
      plain = await decryptAesGcm(
        await cryptoKeyPromise,
        buildFrameIv(salt, parsed.seq),
        parsed.body,
      );
    } catch {
      return; // 单帧解密失败丢弃（连续失败由 transport 层收口）
    }
    if (stopped) return; // 解密期间收口：不得再建 decoder 或解码
    // 解密期间更新帧已提交 → 本帧过期（乱序/迟到帧）。
    if (parsed.seq <= lastSubmittedSeq) return;
    // 重连后的重同步（P2-1）：断流期间中继丢帧，delta 引用缺失数据——只放行关键帧，
    // 否则解码器吃到坏帧整条会话被 error 收口。
    if (awaitingKeyAfterReconnect) {
      if (!parsed.key) {
        opts.onFrameDropped?.();
        return;
      }
      awaitingKeyAfterReconnect = false;
    }
    // seq 缺口（中继/传输/本地背压丢帧）：参考帧链已断，继续解只会出残影。
    // 缺口本身就是关键帧时它即是重同步点，不必再要一枚 IDR。
    if (lastSubmittedSeq >= 0 && parsed.seq > lastSubmittedSeq + 1 && !parsed.key) {
      enterResync();
    }
    if (resyncing && !parsed.key) {
      opts.onFrameDropped?.();
      return;
    }
    if (parsed.key) {
      const codec = codecStringFromAnnexB(plain);
      // decoder 的唯一建立点：首帧（decoder 尚未存在）或 SPS 变化（codec 串变化）才建/换；
      // 重连后的首个关键帧强制重配（forceReconfigureNextKey）——清空旧码流状态防花屏残留。
      // 这里必须在 decode 之前，否则 decoder 永远建不出来（曾因守卫放在本段之前而全程黑屏）。
      if (codec && (decoder === null || codec !== configuredCodec || forceReconfigureNextKey)) {
        configure(codec);
      } else if (!codec && !decoder) {
        // 关键帧没带 SPS 且 decoder 未建立：拿不到 codec 串就无法 configure，只能丢帧；
        // 关键帧 ~2s 一次，warn 便于定位「ready 已发但一直黑屏」。
        logger.warn("[rd-media] keyframe without SPS; decoder not configured");
      }
      forceReconfigureNextKey = false;
      // 关键帧到达即完成重同步（后续 delta 的参考链从这里重新建立）。
      resyncing = false;
    }
    // 非关键帧在 decoder 未配置前丢弃（delta 无法独立解码，等下一个关键帧）。
    if (!decoder || decoder.state !== "configured") return;
    // 解码队列门控：解码器跟不上时进入重同步，只等下一个 IDR；不再「丢一帧、
    // 继续解坏链、再要一枚 IDR」（spec: remote-desktop-latency §2.4）。
    const decodeQueue = decoder.decodeQueueSize ?? 0;
    opts.onDecodeQueue?.(decodeQueue);
    if (shouldDropDeltaFrame(parsed.key, decodeQueue)) {
      opts.onFrameDropped?.();
      enterResync();
      return;
    }
    lastSubmittedSeq = parsed.seq;
    timestampBase += 1_000_000 / 60;
    capturedAtByTimestamp.set(timestampBase, parsed.capturedAtMs);
    decoder.decode(
      new EncodedVideoChunk({
        type: parsed.key ? "key" : "delta",
        timestamp: timestampBase,
        data: plain,
      }),
    );
  };

  /** 幂等收口：ws close/error、decoder error、stop() 任何路径都只通知 onTransportClosed 一次。 */
  function teardown() {
    if (stopped) return;
    stopped = true;
    firstFrameWatchdog.clear();
    if (reconnectTimer !== null) {
      clearTimeout(reconnectTimer);
      reconnectTimer = null;
    }
    capturedAtByTimestamp.clear();
    frameQueue.length = 0;
    resyncing = false;
    try {
      decoder?.close();
    } catch {
      /* 已关闭的 decoder 再 close 抛错无意义：收口路径必须幂等 */
    }
    decoder = null;
    opts.onTransportClosed?.();
  }

  // 初连：连接失败（拒连/证书/服务未就绪）走与瞬断同一条退避重连——每次尝试都会
  // 重新取票，票据过期/消费冲突在重试链上自然恢复。
  attachSocket(openMediaSocket(opts.openSocket, opts.url), false);

  return {
    send(event) {
      const socket = ws;
      if (!socket || socket.readyState !== MEDIA_SOCKET_OPEN) return;
      // 幂等补发保险：正常路径 ready 已在 onopen 发出；万一 open 事件派发晚于本调用，
      // 这里补一次（ready 必须恰好一次：helper 收到前的帧不推流，重复 ready 会让 host 重下发开流指令）。
      if (!readySent) {
        socket.send(JSON.stringify({ t: "ready" }));
        readySent = true;
      }
      socket.send(JSON.stringify(event));
    },
    isOpen() {
      const socket = ws;
      return socket !== null && socket.readyState === MEDIA_SOCKET_OPEN;
    },
    ping(ts) {
      const socket = ws;
      if (socket && socket.readyState === MEDIA_SOCKET_OPEN) {
        socket.send(JSON.stringify({ t: "ping", ts }));
      }
    },
    setClockOffset(offsetMs) {
      clockOffsetMs = offsetMs;
    },
    stop() {
      // 本端主动收口：先作废退避重连（否则 close 事件会再排一轮）。
      if (reconnectTimer !== null) {
        clearTimeout(reconnectTimer);
        reconnectTimer = null;
      }
      try {
        ws?.close(1000, "client-stop");
      } catch {
        /* CONNECTING 状态下 close 在部分环境抛错：stop 必须幂等 */
      }
      // 主动 stop 也会使 onTransportClosed 通知一次（任何路径收口都只通知一次，见 teardown）：
      // 消费方按幂等处理，不要把自身发起的 stop 误记成传输故障。
      teardown();
    },
  };
}

async function decryptAesGcm(
  cryptoKey: CryptoKey,
  iv: Uint8Array,
  body: Uint8Array,
): Promise<Uint8Array> {
  // key 在会话建立时导入一次（原实现逐帧 importKey：60fps 下每秒 60 次无谓的异步导入）。
  // body = ciphertext ‖ tag(16)（WebCrypto 约定 tag 附尾）
  const plain = await crypto.subtle.decrypt(
    { name: "AES-GCM", iv: asBufferSource(iv) },
    cryptoKey,
    asBufferSource(body),
  );
  return new Uint8Array(plain);
}

/**
 * WebCrypto 的 BufferSource 在 TS6 的类型面排除了 SharedArrayBuffer 视图，而 TS 无法从
 * `Uint8Array<ArrayBufferLike>` 推出底层不是 SharedArrayBuffer。帧/key/盐都来自 WebSocket 与本地
 * 构造，按 byteOffset/byteLength 重建视图只做类型收窄、不复制数据（逐帧热路径）。
 */
function asBufferSource(bytes: Uint8Array): Uint8Array<ArrayBuffer> {
  return new Uint8Array(bytes.buffer as ArrayBuffer, bytes.byteOffset, bytes.byteLength);
}

/** 入口校验长度与字符集：坏 key/salt 只会让每帧在解密处静默失败，构造期抛错才能暴露配置问题。 */
function hexToBytes(hex: string, expectedBytes: number, label: string): Uint8Array {
  if (hex.length !== expectedBytes * 2 || !/^[0-9a-f]+$/i.test(hex)) {
    throw new Error(`invalid ${label}: expected ${expectedBytes * 2} hex chars`);
  }
  const out = new Uint8Array(expectedBytes);
  for (let i = 0; i < expectedBytes; i++) {
    out[i] = Number.parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}
