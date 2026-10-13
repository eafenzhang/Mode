/**
 * 原生媒体面 renderer 会话（spec: docs/specs/lan-remote-desktop-native-media.md）。
 * 纯函数（帧头/IV/SPS）不碰 DOM；胶水部分用 WebCodecs + WS（仅桌面 Chromium）。
 *
 * 下行帧布局（契约 B v2，helper 生产、这里消费，逐字节一致）：
 *   [u32 seq 大端][u8 flags(bit0=关键帧)][u32 capturedAtMs（被控时钟，帧龄起点）][ciphertext ‖ GCM tag(16)]
 * IV 不随帧传：iv = salt(8B) ∥ seq(4B 大端)，aes-256-gcm（spec 安全节）。
 */
import { logger } from "@/logger.js";
import { nextReconnectDelayMs, RECONNECT_MAX_ATTEMPTS } from "@/remote-desktop/reconnectPolicy.js";
import { codecStringFromAnnexB } from "./codecString.js";

// codec 串派生与 HEVC 解码预检拆至 codecString.ts（max-lines 约束）；
// re-export 保持既有消费方（会话胶水/测试）导入路径不变。
export { codecStringFromAnnexB, probeHevcDecodeSupport } from "./codecString.js";

export interface ParsedMediaFrame {
  seq: number;
  key: boolean;
  /** 采集时刻（被控时钟 Unix 毫秒 u32 截断；0 = 未知）。帧龄 = now − capturedAtMs + 时钟偏移。 */
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
 * delta 帧在解码排队 >2 时丢弃（解码跟不上到达时继续排队只会累计端到端延迟）；
 * 关键帧永远放行——它是丢帧后的重同步点。丢弃方需上报 need-key（调用点在会话胶水）。
 */
export function shouldDropDeltaFrame(isKey: boolean, decodeQueueSize: number): boolean {
  return !isKey && decodeQueueSize > 2;
}

export interface NativeMediaSessionOptions {
  url: string;
  keyHex: string;
  saltHex: string;
  canvas: HTMLCanvasElement;
  onConfigured?: (info: { width: number; height: number; codec: string }) => void;
  onDecoded?: () => void;
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
  // 帧龄：提交时按 chunk 时间戳记采集时刻，绘制时领回算出端到端年龄；
  // 时钟偏移由 pong 标定（未标定前只跳过帧龄读数，画面不受影响）。
  const capturedAtByTimestamp = new Map<number, number>();
  let clockOffsetMs: number | null = null;
  // 媒体 WS 瞬断重连（P2-1）状态：当前 socket、退避计数/定时器、重连后的重同步标志
  //（断流期间中继丢帧 → 只放行关键帧并强制重配解码器，防花屏残留）。
  let ws: WebSocket | null = null;
  let reconnectAttempts = 0;
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  let awaitingKeyAfterReconnect = false;
  let forceReconfigureNextKey = false;

  /** 紧急关键帧请求（丢帧恢复/重连重同步）：ws 上行 {t:"need-key"}；500ms 去抖
   *  （中继侧另有 300ms 去抖）；force = 重连路径绕过去抖（断流后的第一诉求）。 */
  const requestKey = (force = false): void => {
    const now = Date.now();
    if (!force && now - lastKeyRequestAt < 500) {
      return;
    }
    lastKeyRequestAt = now;
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify({ t: "need-key" }));
    }
  };

  /** 退避耗尽/无重连能力：按永久失败收口（与旧行为一致）。 */
  const scheduleReconnect = (): void => {
    reconnectAttempts += 1;
    if (reconnectAttempts > RECONNECT_MAX_ATTEMPTS || !opts.onRequestMediaUrl) {
      teardown();
      return;
    }
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
    attachSocket(new WebSocket(url), true);
  };

  /**
   * socket 挂接（初连/重连共用）：所有 handler 都做 `ws === socket` 身份核对，
   * 被顶替/收口后的迟到事件不驱动状态。close 非本端收口 → 进入退避重连。
   */
  const attachSocket = (socket: WebSocket, isReconnect: boolean): void => {
    ws = socket;
    socket.binaryType = "arraybuffer";
    socket.onopen = () => {
      if (ws !== socket || stopped) {
        return;
      }
      if (isReconnect) {
        // 重连成功：重置退避；断流期间丢帧使 decoder 状态过时——只放行关键帧，
        // 首个关键帧强制重配解码器（防花屏残留）；立刻请关键帧缩短重同步时间。
        reconnectAttempts = 0;
        awaitingKeyAfterReconnect = true;
        forceReconfigureNextKey = true;
        requestKey(true);
        opts.onReconnecting?.(false);
      }
      if (!readySent) {
        // 幂等补发保险（初连）：ready 必须恰好一次。重连不需要重发——中继的 ready 门控
        // 是每条会话一次，重复 ready 会被中继幂等忽略。
        socket.send(JSON.stringify({ t: "ready" }));
        readySent = true;
      }
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
        opts.onDecoded?.();
        // 帧龄 = 本端绘制时刻 − 被控采集时刻 + 时钟偏移（glass-to-glass 近似）；
        // 缺采集戳（旧端/未知=0）或未标定偏移 → 跳过本帧读数。
        if (
          typeof capturedAtMs === "number" &&
          capturedAtMs !== 0 &&
          clockOffsetMs !== null
        ) {
          opts.onFrameAgeMs?.(Date.now() - capturedAtMs + clockOffsetMs);
        }
      },
      error: (e) => {
        logger.warn("[rd-media] decoder error:", e);
        teardown();
      },
    });
    try {
      decoder.configure({ codec, optimizeForLatency: true });
    } catch (error) {
      // codec 串不被浏览器接受（HEVC 派生串异常/平台不支持）：按媒体面失败收口，
      // 不留黑屏死等（teardown → onTransportClosed → 会话失败归因）。
      logger.warn("[rd-media] decoder configure failed:", error);
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
  // ws 一开就发 ready：helper 的 ready 门控只认第一条开流指令，而被控状态机保证首帧必为 IDR
  // （先 IDR 后 delta），所以不必等任何上行事件——否则画面要等用户首次移动鼠标/按键才出流，开局全黑。
  // （ready 发送在 attachSocket 的 onopen 里，初连与重连共用同一幂等位。）
  const handleMessage = (event: MessageEvent): void => {
    if (typeof event.data === "string") {
      let msg: { t?: string };
      try {
        msg = JSON.parse(event.data) as { t?: string };
      } catch {
        // 下行 JSON 是协议内数据：解析失败按单条丢弃，不让坏数据变成未捕获异常。
        return;
      }
      if (msg.t === "pong" || msg.t === "freeze" || msg.t === "unfreeze") {
        // freeze/unfreeze/pong 通道：Task 10/13 订阅（替代已删除的 reducer 信号直染）。
        window.dispatchEvent(new CustomEvent("rd-media-control", { detail: msg }));
      }
      return;
    }
    const parsed = parseFrameHeader(new Uint8Array(event.data as ArrayBuffer));
    // 收口后不再处理任何帧：decoder error 与 ws error 路径不关 socket（只等消费方 stop），
    // 缺了 stopped 判据，收口后的关键帧会重新建 decoder 复活解码。
    if (!parsed || stopped) return;
    // 码率读数：按收到的密文字节数计（含帧头与 GCM tag，≈ 实际占线带宽）。
    opts.onBytesReceived?.((event.data as ArrayBuffer).byteLength);
    // 防乱序/迟到：解密是异步的，提交前先按 seq 水位过滤（廉价前置，省一次解密）。
    if (parsed.seq <= lastSubmittedSeq) return;
    void (async () => {
      let plain: Uint8Array;
      try {
        plain = await decryptAesGcm(await cryptoKeyPromise, buildFrameIv(salt, parsed.seq), parsed.body);
      } catch {
        return; // 单帧解密失败丢弃（连续失败由 transport 层收口）
      }
      if (stopped) return; // 解密期间收口：不得再建 decoder 或解码
      // 解密期间更新帧已提交 → 本帧过期（异步完成顺序无保证）。
      if (parsed.seq <= lastSubmittedSeq) return;
      // 重连后的重同步（P2-1）：断流期间中继丢帧，delta 引用缺失数据——只放行关键帧，
      // 否则解码器吃到坏帧整条会话被 error 收口。
      if (awaitingKeyAfterReconnect) {
        if (!parsed.key) {
          return;
        }
        awaitingKeyAfterReconnect = false;
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
      }
      // 非关键帧在 decoder 未配置前丢弃（delta 无法独立解码，等下一个关键帧）。
      if (!decoder || decoder.state !== "configured") return;
      // 解码队列门控：跟不上到达的 delta 丢弃并请关键帧（继续排队 = 累计端到端延迟）。
      const queueSize = decoder.decodeQueueSize;
      opts.onDecodeQueue?.(queueSize);
      if (shouldDropDeltaFrame(parsed.key, queueSize)) {
        opts.onFrameDropped?.();
        requestKey();
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
    })().catch(() => teardown());
  };

  /** 幂等收口：ws close/error、decoder error、stop() 任何路径都只通知 onTransportClosed 一次。 */
  function teardown() {
    if (stopped) return;
    stopped = true;
    if (reconnectTimer !== null) {
      clearTimeout(reconnectTimer);
      reconnectTimer = null;
    }
    capturedAtByTimestamp.clear();
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
  attachSocket(new WebSocket(opts.url), false);

  return {
    send(event) {
      const socket = ws;
      if (!socket || socket.readyState !== WebSocket.OPEN) return;
      // 幂等补发保险：正常路径 ready 已在 onopen 发出；万一 open 事件派发晚于本调用，
      // 这里补一次（ready 必须恰好一次：helper 收到前的帧不推流，重复 ready 会让 host 重下发开流指令）。
      if (!readySent) {
        socket.send(JSON.stringify({ t: "ready" }));
        readySent = true;
      }
      socket.send(JSON.stringify(event));
    },
    ping(ts) {
      const socket = ws;
      if (socket && socket.readyState === WebSocket.OPEN) {
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
