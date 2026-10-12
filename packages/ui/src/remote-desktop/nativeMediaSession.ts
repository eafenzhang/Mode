/**
 * 原生媒体面 renderer 会话（spec: docs/specs/lan-remote-desktop-native-media.md）。
 * 纯函数（帧头/IV/SPS）不碰 DOM；胶水部分用 WebCodecs + WS（仅桌面 Chromium）。
 *
 * 下行帧布局（契约 B，helper 生产、这里消费，逐字节一致）：
 *   [u32 seq 大端][u8 flags(bit0=关键帧)][ciphertext ‖ GCM tag(16)]
 * IV 不随帧传：iv = salt(8B) ∥ seq(4B 大端)，aes-256-gcm（spec 安全节）。
 */
import { logger } from "@/logger.js";

export interface ParsedMediaFrame {
  seq: number;
  key: boolean;
  body: Uint8Array;
}

/** 切帧头：不足 5 字节（残包）返回 null，由调用方丢弃。 */
export function parseFrameHeader(bytes: Uint8Array): ParsedMediaFrame | null {
  if (bytes.length < 5) return null;
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  return {
    seq: view.getUint32(0, false),
    key: (bytes[4]! & 1) === 1,
    body: bytes.subarray(5),
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
 * Annex-B 里找首个 SPS（NAL type 7）→ "avc1.<profile hex><constraint hex><level hex>"。
 * 关键帧里的 SPS 是分辨率/编码参数变化的唯一信号（spec：新 SPS 触发 decoder 重配）。
 */
export function codecStringFromAnnexB(bytes: Uint8Array): string | null {
  // 扫描 4/3 字节起始码，取 type = byte & 0x1f === 7 的首段
  for (let i = 0; i + 5 < bytes.length; i++) {
    const is4 = bytes[i] === 0 && bytes[i + 1] === 0 && bytes[i + 2] === 0 && bytes[i + 3] === 1;
    const is3 = !is4 && bytes[i] === 0 && bytes[i + 1] === 0 && bytes[i + 2] === 1;
    if (!is4 && !is3) continue;
    const headerIdx = i + (is4 ? 4 : 3);
    if ((bytes[headerIdx]! & 0x1f) !== 7 || headerIdx + 3 >= bytes.length) continue;
    const hex = (n: number) => n.toString(16).padStart(2, "0");
    return `avc1.${hex(bytes[headerIdx + 1]!)}${hex(bytes[headerIdx + 2]!)}${hex(bytes[headerIdx + 3]!)}`;
  }
  return null;
}

export interface NativeMediaSessionOptions {
  url: string;
  keyHex: string;
  saltHex: string;
  canvas: HTMLCanvasElement;
  onConfigured?: (info: { width: number; height: number; codec: string }) => void;
  onDecoded?: () => void;
  onTransportClosed?: () => void;
}

export interface NativeMediaSession {
  /** ready 由 ws open 发出（首条 send 亦作幂等补发保险）；随后原样上行事件对象。 */
  send(event: object): void;
  ping(ts: number): void;
  stop(): void;
}

export function createNativeMediaSession(opts: NativeMediaSessionOptions): NativeMediaSession {
  const key = hexToBytes(opts.keyHex, 32, "key");
  const salt = hexToBytes(opts.saltHex, 8, "salt");
  const ws = new WebSocket(opts.url);
  ws.binaryType = "arraybuffer";
  const ctx = opts.canvas.getContext("2d", { alpha: false });
  if (!ctx) throw new Error("2d canvas unavailable");
  let decoder: VideoDecoder | null = null;
  let configuredCodec: string | null = null;
  let readySent = false;
  let stopped = false;
  // 编码器不给真实时间戳：EncodedVideoChunk 只要求时间戳单调，按 60fps 名义步长递增即可
  // （该值不参与绘制与统计；fps = 实际绘制帧率，由 onDecoded 侧统计）。
  let timestampBase = 0;

  const configure = (codec: string) => {
    decoder?.close();
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
        frame.close();
        opts.onDecoded?.();
      },
      error: (e) => {
        logger.warn("[rd-media] decoder error:", e);
        teardown();
      },
    });
    decoder.configure({ codec, optimizeForLatency: true });
    configuredCodec = codec;
  };

  // ws 一开就发 ready：helper 的 ready 门控只认第一条开流指令，而被控状态机保证首帧必为 IDR
  // （先 IDR 后 delta），所以不必等任何上行事件——否则画面要等用户首次移动鼠标/按键才出流，开局全黑。
  ws.onopen = () => {
    // send() 可能在 readyState 已 OPEN 但 open 事件尚未派发的窗口里先发过：保持「ready 恰一次」。
    if (readySent) return;
    ws.send(JSON.stringify({ t: "ready" }));
    readySent = true;
  };

  ws.onmessage = (event) => {
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
    if (!parsed || !decoder) return;
    void (async () => {
      let plain: Uint8Array;
      try {
        plain = await decryptAesGcm(key, buildFrameIv(salt, parsed.seq), parsed.body);
      } catch {
        return; // 单帧解密失败丢弃（连续失败由 transport 层收口）
      }
      if (parsed.key) {
        const codec = codecStringFromAnnexB(plain);
        if (codec && codec !== configuredCodec) configure(codec);
      }
      if (!decoder || decoder.state !== "configured") return;
      timestampBase += 1_000_000 / 60;
      decoder.decode(
        new EncodedVideoChunk({
          type: parsed.key ? "key" : "delta",
          timestamp: timestampBase,
          data: plain,
        }),
      );
    })();
  };
  ws.onclose = () => teardown();
  ws.onerror = () => teardown();

  /** 幂等收口：ws close/error、decoder error、stop() 任何路径都只通知 onTransportClosed 一次。 */
  function teardown() {
    if (stopped) return;
    stopped = true;
    try {
      decoder?.close();
    } catch {
      /* 已关闭的 decoder 再 close 抛错无意义：收口路径必须幂等 */
    }
    decoder = null;
    opts.onTransportClosed?.();
  }

  return {
    send(event) {
      if (ws.readyState !== WebSocket.OPEN) return;
      // 幂等补发保险：正常路径 ready 已在 onopen 发出；万一 open 事件派发晚于本调用，
      // 这里补一次（ready 必须恰好一次：helper 收到前的帧不推流，重复 ready 会让 host 重下发开流指令）。
      if (!readySent) {
        ws.send(JSON.stringify({ t: "ready" }));
        readySent = true;
      }
      ws.send(JSON.stringify(event));
    },
    ping(ts) {
      if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ t: "ping", ts }));
    },
    stop() {
      try {
        ws.close(1000, "client-stop");
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
  key: Uint8Array,
  iv: Uint8Array,
  body: Uint8Array,
): Promise<Uint8Array> {
  const cryptoKey = await crypto.subtle.importKey("raw", asBufferSource(key), "AES-GCM", false, [
    "decrypt",
  ]);
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
