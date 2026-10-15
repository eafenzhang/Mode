// 远程桌面媒体泵：契约 A（napi 采集面）与契约 B（本地管道）之间的 helper 侧适配。
//   helper→host：kind0 帧 = [u32 seq][u8 flags(bit0=关键帧)][u32 capturedAtMs(Unix 毫秒
//                u32 截断；0=未知 — 帧龄起点，主控侧 pong 标定时钟偏移后算端到端帧龄)]
//                [ciphertext‖tag(16B)]，逐帧 AES-256-GCM；IV 不随帧传，key 32B、
//                IV = salt(8B) ∥ seq(4B 大端)（spec 安全节）。
//                kind1 状态 = UTF-8 JSON {"frozen":bool[,"reason":string]}（host 转 freeze/unfreeze 信令）。
//                kind5 剪贴板 = UTF-8 文本（本端剪贴板**变化**时上报；语义门在 host，见下）。
//   host→helper：kind2 开流指令 = [u32BE 1][0x02]；**ready 门控**——收到前只采集不推流
//                （避免中继在 media WS 未就绪时缓冲堆积）。
//                kind3 紧急关键帧 = [u32BE 1][0x03]（renderer 解码落后 / 中继背压丢帧时请求；
//                只读 flag 语义、无 payload；ready 前忽略）。
//                kind4 动态码率 = [u32BE 5][0x04][u32BE bps]（自适应链路的下行方向盘；ready 前忽略）。
//                kind6 剪贴板写入 = [u32BE 1+n][0x06][UTF-8 文本]（host 采纳主控文本后要求落到
//                本端剪贴板；写入失败只丢本次，下一次轮询会重新上报真实值）。
// 分帧总口径：[u32 len 大端][u8 kind][payload]，len = 1 + payload.length（与 host 中继同契约）。
import { createCipheriv } from "node:crypto";
import { connect } from "node:net";

import { parseAxError } from "./errors.mjs";

const invalid = (message) => Object.assign(new Error(message), { code: "invalid_request" });

// 入站 payload 上限（kind6 的剪贴板文本要放进这一条消息）：文本上限是 128K UTF-16 码元
// （shared 侧 schema），UTF-8 最坏 3 字节/码元 ≈ 384 KiB，留一倍余量。超过即按协议破坏弃缓冲。
const MAX_INBOX_PAYLOAD = 512 * 1024;

// 剪贴板轮询间隔：文本同步是便利功能，700ms 对「复制→粘贴」的体感足够，
// 又不会让 OpenClipboard 争用变成常态。
const CLIPBOARD_POLL_MS = 700;

// 与 server.mjs callAddon 同口径：napi 抛 "<code>:<msg>" 串 → parseAxError 拆 17 码重缠
// （拆不出前缀归 internal，保原文诊断）；本模块的 invalid_request 在 addon 调用之外抛出，
// 不走这里（无前缀的校验文案会被 parseAxError 误归 internal）。
async function callAddon(fn) {
  try {
    return await fn();
  } catch (error) {
    const { code, message } = parseAxError(error);
    throw Object.assign(new Error(message), { code });
  }
}

function onceConnect(socket) {
  return new Promise((resolve, reject) => {
    socket.once("connect", resolve);
    socket.once("error", reject);
  });
}

function u32be(n) {
  const b = Buffer.alloc(4);
  b.writeUInt32BE(n >>> 0, 0);
  return b;
}

export function createMediaPump(addon, options = {}) {
  /** 剪贴板轮询间隔：可在测试里注入更短的周期（生产用缺省值）。 */
  const clipboardPollMs = options.clipboardPollMs ?? CLIPBOARD_POLL_MS;
  /** 当前会话（null = 未启动）；回调一律按会话身份过滤，见 emitFrame 的迟到投递说明。 */
  let state = null;

  async function stop() {
    const session = state;
    if (!session) return { ok: true }; // 幂等（契约 G）：未启动/已收口都安全返回
    state = null; // 先摘身份：TSFN 队列里迟到的帧回调立即失效，stop 后不得再写管道
    if (session.clipboardTimer) clearInterval(session.clipboardTimer);
    try {
      await callAddon(() => addon.rd_media_stop());
    } catch {
      // 幂等收口：napi stop 对「未启动」不报错（Task 2）；真错误也不阻断管道收口。
    }
    session.socket.destroy();
    return { ok: true };
  }

  async function start(params) {
    await stop(); // 抢占式：重复 start 先停旧泵（旧 addon 会话 + 旧管道），再建新会话
    const key = Buffer.from(params.key, "hex");
    const salt = Buffer.from(params.salt, "hex");
    if (key.length !== 32) throw invalid("key must be 32 bytes (64 hex chars)");
    if (salt.length !== 8) throw invalid("salt must be 8 bytes (16 hex chars)");

    const socket = connect({ host: params.endpoint.host, port: params.endpoint.port, noDelay: true });
    // 常驻 error 监听：会话中途断管（host 收口/重置）不能被 Node 当 uncaught 'error'
    // 抛出——helper 进程要活到下一会话；连接期的错误仍由 onceConnect 如实 reject。
    socket.on("error", () => {});
    try {
      await onceConnect(socket);
    } catch (error) {
      socket.destroy();
      throw error; // 连接失败（ECONNREFUSED 等）原样 reject → broker 错误响应
    }

    const session = { socket, key, salt, ready: false };
    state = session;
    // host→helper 方向的控制消息（契约 B）：[u32 len][kind][payload?]。TCP 分片/粘包按
    // 长度前缀攒包；半包必须容忍（漏掉 kind2 会静默不推流）。len/kind/payload 形状按
    // 契约逐 kind 校验：长度失真视为协议破坏，弃缓冲重来。
    let inbox = Buffer.alloc(0);
    socket.on("data", (chunk) => {
      inbox = inbox.length === 0 ? chunk : Buffer.concat([inbox, chunk]);
      while (inbox.length >= 4) {
        const len = inbox.readUInt32BE(0);
        if (len < 1 || len > 1 + MAX_INBOX_PAYLOAD) {
          inbox = Buffer.alloc(0); // 长度越界：协议破坏，弃缓冲重来
          return;
        }
        if (inbox.length < 4 + len) {
          return; // 半包：等后续 chunk 补齐
        }
        const body = inbox.subarray(4, 4 + len);
        inbox = inbox.subarray(4 + len);
        const kind = body[0];
        if (kind === 2 && body.length === 1) {
          session.ready = true; // 开流指令：解除 ready 门控（仅当前会话生效）
        } else if (kind === 3 && body.length === 1 && session.ready) {
          // 紧急关键帧：napi 侧一次性消费（只读 flag）；异常不抛穿（Fatal 策略下会杀进程）。
          try {
            addon.rd_media_request_key?.();
          } catch {
            /* 单次控制面失败不影响推流 */
          }
        } else if (kind === 4 && body.length === 5 && session.ready) {
          // 动态码率：napi 侧记录，worker 下一 tick 消费（ICodecAPI 直接改 MeanBitRate）。
          const bps = body.readUInt32BE(1);
          if (bps > 0) {
            try {
              addon.rd_media_set_bitrate?.(bps);
            } catch {
              /* 同上：失败只丢本次调整，保持原码率 */
            }
          }
        } else if (kind === 6 && body.length > 1 && session.ready) {
          void applyClipboardText(session, body.subarray(1).toString("utf8"));
        }
      }
    });

    let startResult;
    try {
      startResult = await callAddon(() =>
        addon.rd_media_start({
          rect: params.rect,
          maxWidth: params.maxWidth,
          fps: params.fps,
          bitrateBps: params.bitrateBps,
          // 编码 codec 偏好（HEVC 选型）：透传给 napi（Option<String>，undefined = h264）。
          codec: params.codec,
          onFrame: (frame) => emitFrame(session, frame),
          onStatus: (status) => emitStatus(session, status),
        }),
      );
    } catch (error) {
      // 起采集失败：不留半开会话——host 中继单 helper 槽位不能被死连接占住（旧连接会
      // 消费后续 openRequested 的开流指令），下一次 start 也不该面对陈旧管道。
      // 守卫 state === session：并发 stop/start 已换掉会话时不误清后来者。
      if (state === session) state = null;
      socket.destroy();
      throw error;
    }
    // 剪贴板同步：基线 = 会话建立时本端已有内容——**同步从「变化」开始**，连接本身不该把
    // 对端的既有剪贴板顶掉（那是打扰，不是同步）。基线取不到（争用/无文本）就留 null，
    // 此时第一次变化照常上报，语义正确。
    session.lastClipboard = await readClipboardText();
    session.clipboardTimer = setInterval(() => void pollClipboard(session), clipboardPollMs);
    // 编码器来源（spec「软件编码回退」2026-10-15）：原样回报给 host 判能力
    // （"hardware" | "software"）。napi 未回报（旧 addon）时字段缺席 = 不猜测。
    return { ok: true, ...(startResult?.encoder ? { encoder: startResult.encoder } : {}) };
  }

  async function switchMonitor(params) {
    // 契约 A 的 napi 面是 rd_media_switch(rect)：rect 本体直接做实参（napi RdMediaRect
    // 四字段 x/y/width/height）；契约 G 的 broker 参数是 {rect}，这里拆包——若按简报
    // 字面把整个 params 透传，napi 会因缺 x/y/width/height 抛转换错误。
    await callAddon(() => addon.rd_media_switch(params.rect));
    return { ok: true };
  }

  // 帧回调（TSFN ErrorStrategy::Fatal：回调里抛异常会中止整个 helper 进程）：
  // 一切失败路径只丢帧，绝不向外抛。
  function emitFrame(session, frame) {
    // 会话身份 + ready 门控（契约 B）：kind2 前的帧、stop 后/旧会话的 TSFN 迟到投递都不得写。
    if (state !== session || !session.ready) return;
    const seq = frame.seq >>> 0;
    const iv = Buffer.concat([session.salt, u32be(seq)]); // IV = salt(8B) ∥ seq(4B 大端)
    const cipher = createCipheriv("aes-256-gcm", session.key, iv);
    const ct = Buffer.concat([cipher.update(Buffer.from(frame.data)), cipher.final()]);
    // payload = [u32 seq][u8 flags][u32 capturedAtMs][ciphertext‖tag]：头 9 字节（契约 B v2）
    const payload = Buffer.alloc(9 + ct.length + 16);
    payload.writeUInt32BE(seq, 0);
    payload.writeUInt8(frame.key ? 1 : 0, 4);
    payload.writeUInt32BE((frame.capturedAtMs ?? 0) >>> 0, 5);
    ct.copy(payload, 9);
    cipher.getAuthTag().copy(payload, 9 + ct.length);
    writeKind(session, 0, payload);
  }

  // 状态事件（kind1）：不受 ready 门控——推流前也要能把 freeze 交给 host 转信令。
  // JSON 直接序列化 napi 回调载荷：frozen 恒在；reason 缺席/为 null 都合法（host 侧可空解析）。
  function emitStatus(session, status) {
    if (state !== session) return;
    writeKind(session, 1, Buffer.from(JSON.stringify(status), "utf8"));
  }

  // 剪贴板读取（kind5 的数据源）：失败只代表「本 tick 没读到」（OpenClipboard 被别的进程
  // 短暂占用是常态），不当作会话故障、不重试——下一次轮询自然覆盖。
  async function readClipboardText() {
    try {
      const text = await addon.clipboard_read_text?.();
      return typeof text === "string" && text.length > 0 ? text : null;
    } catch {
      return null;
    }
  }

  // 剪贴板轮询（helper→host kind5）：**只做「变化才发」的流量抑制**，语义门（回环抑制与
  // 开关）归 host——一处拥有，避免两端各持一份判据而漂移。
  async function pollClipboard(session) {
    if (state !== session) return;
    const text = await readClipboardText();
    if (text === null || text === session.lastClipboard) return;
    session.lastClipboard = text;
    writeKind(session, 5, Buffer.from(text, "utf8"));
  }

  // 主控文本落到本端剪贴板（host→helper kind6）：写成功后登记 lastClipboard，
  // 否则本端轮询会把刚写进去的值当成「本端变化」再报一次（对端虽会忽略，但没必要）。
  // 写失败不登记——下一次轮询上报真实值，让 host 的门看到「期望值与实际值不一致」。
  async function applyClipboardText(session, text) {
    try {
      await addon.clipboard_write_text?.(text);
      session.lastClipboard = text;
    } catch {
      /* 写入失败：留给下一次轮询纠正，不打断会话 */
    }
  }

  function writeKind(session, kind, payload) {
    const socket = session.socket;
    if (socket.destroyed) return;
    const header = Buffer.alloc(5);
    header.writeUInt32BE(1 + payload.length, 0); // len = 1（kind 字节）+ payload.length
    header.writeUInt8(kind, 4);
    try {
      socket.write(Buffer.concat([header, payload]));
    } catch {
      // 管道已断（写竞态）：丢帧不抛穿——TSFN Fatal 策略下抛出会中止 helper 进程。
    }
  }

  return { start, switchMonitor, stop };
}
