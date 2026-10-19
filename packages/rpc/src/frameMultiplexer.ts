/**
 * 单端口多路复用（LR-4 · DR-LR-2 的落地原语）。
 *
 * Tauri 桥只有一条页面 ↔ 壳 ↔ Host 的二进制帧管道，而 remote workspace 的每个 logical
 * session 需要独立的 service scope。本模块给 remote scope 的帧加一个**保留首字节 + u32
 * 大端 scope id** 的前缀；base scope 的帧逐字节不变（RequestType 从 100 起、
 * ResponseType 从 200 起，0xFF 与现有帧类型不冲突）。
 *
 * Rust 壳只搬字节，不需要知道 scope；解复用只发生在页面桥与 Host。
 * 设计与失败模式：`docs/plans/2026-10-19-lr4-attachment-spike.md`。
 */
import { Emitter } from "./foundation.js";
import type { IMessagePassingProtocol } from "./protocol.js";
import { VSBuffer } from "./buffer.js";

/** 保留首字节：任何合法 RPC 帧的首字节都不会是 0xFF。 */
export const SCOPED_FRAME_MARKER = 0xff;
/** 控制帧（页面桥 → Host，不属于任何 RPC scope）：scope-ready / scope-detach。 */
export const CONTROL_FRAME_MARKER = 0xfe;
/** marker(1) + scopeId(4, BE)。 */
export const SCOPED_FRAME_HEADER_BYTES = 5;
/** scope id 的有效范围：0 保留（base 不带前缀），正整数到 u32 上限。 */
export const MAX_SCOPE_ID = 0xffff_ffff;

/** 页面桥 → Host 的附件控制帧；必须早于 RPC 帧校验处理。 */
export type AttachmentControl =
  | { type: "scope-ready"; scopeId: number }
  | { type: "scope-detach"; scopeId: number };

function assertScopeId(scopeId: number): void {
  if (!Number.isInteger(scopeId) || scopeId < 1 || scopeId > MAX_SCOPE_ID) {
    throw new RangeError(`scope id 必须是 1..${MAX_SCOPE_ID} 的整数：${scopeId}`);
  }
}

/** 编码控制帧：`[0xFE][JSON utf8]`。控制帧不承载 RPC，只走页面桥与 Host。 */
export function encodeControlFrame(control: AttachmentControl): Uint8Array {
  assertScopeId(control.scopeId);
  if (control.type !== "scope-ready" && control.type !== "scope-detach") {
    throw new Error(`未知附件控制：${String((control as { type?: unknown }).type)}`);
  }
  const body = new TextEncoder().encode(JSON.stringify(control));
  const out = new Uint8Array(1 + body.byteLength);
  out[0] = CONTROL_FRAME_MARKER;
  out.set(body, 1);
  return out;
}

/** 解析控制帧；不是控制帧返回 null，控制帧畸形也返回 null（调用方按坏帧丢弃）。 */
export function decodeControlFrame(frame: Uint8Array): AttachmentControl | null {
  if (frame.byteLength < 2 || frame[0] !== CONTROL_FRAME_MARKER) {
    return null;
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(new TextDecoder().decode(frame.subarray(1)));
  } catch {
    return null;
  }
  if (!parsed || typeof parsed !== "object") {
    return null;
  }
  const type = (parsed as { type?: unknown }).type;
  const scopeId = (parsed as { scopeId?: unknown }).scopeId;
  if (type !== "scope-ready" && type !== "scope-detach") {
    return null;
  }
  if (typeof scopeId !== "number" || !Number.isInteger(scopeId) || scopeId < 1) {
    return null;
  }
  return { type, scopeId };
}

export interface DecodedFrame {
  /** null = base scope。 */
  scopeId: number | null;
  payload: Uint8Array;
}

/**
 * 编码一个 remote scope 帧。非法 scope id 直接抛（分配方在 Host，错误应尽早暴露）。
 * 返回值是新分配的数组，不共享入参的底层 buffer。
 */
export function encodeScopedFrame(scopeId: number, frame: Uint8Array): Uint8Array {
  assertScopeId(scopeId);
  const out = new Uint8Array(SCOPED_FRAME_HEADER_BYTES + frame.byteLength);
  out[0] = SCOPED_FRAME_MARKER;
  new DataView(out.buffer, out.byteOffset, out.byteLength).setUint32(1, scopeId, false);
  out.set(frame, SCOPED_FRAME_HEADER_BYTES);
  return out;
}

/**
 * 解析一帧。返回 `null` = 畸形 scoped 头（marker 在但不足 5 字节），调用方必须丢弃；
 * 合法 base 帧返回 `{ scopeId: null, payload }`（payload 原样，不复制）。
 */
export function decodeFrame(frame: Uint8Array): DecodedFrame | null {
  if (frame.byteLength === 0 || frame[0] !== SCOPED_FRAME_MARKER) {
    return { scopeId: null, payload: frame };
  }
  if (frame.byteLength < SCOPED_FRAME_HEADER_BYTES) {
    return null;
  }
  const view = new DataView(frame.buffer, frame.byteOffset, frame.byteLength);
  return {
    scopeId: view.getUint32(1, false),
    payload: frame.subarray(SCOPED_FRAME_HEADER_BYTES),
  };
}

export interface FrameDemultiplexerOptions {
  /** 未知/已回收 scope 的帧只计数并上报（不误投 base）。 */
  onUnknownScope?: (scopeId: number) => void;
  /** 畸形 scoped 头（marker 在但长度不足）。 */
  onInvalidFrame?: () => void;
}

export interface FrameDemultiplexer {
  /** base scope：发出的帧不带前缀、收到的帧只含 base 帧。 */
  readonly base: IMessagePassingProtocol;
  /** 绑定一个 remote scope；重复绑定同一 id 是配置错误，直接抛。 */
  attachScope(scopeId: number): IMessagePassingProtocol;
  /** 回收 scope：之后的帧按未知 scope 处理。 */
  detachScope(scopeId: number): void;
  readonly unknownScopeFrames: number;
  readonly invalidFrames: number;
  dispose(): void;
}

/**
 * 在一条物理传输上建立 base + 动态 scope 的解复用。
 *
 * 生命周期：构造时订阅传输一次；`attachScope` 之后该 scope 的帧才会被投递；
 * `detachScope` 关闭对应 emitter，之后的帧计数丢弃。传输本身的关闭由调用方负责。
 */
export function createFrameDemultiplexer(
  transport: IMessagePassingProtocol,
  options: FrameDemultiplexerOptions = {},
): FrameDemultiplexer {
  const baseEmitter = new Emitter<VSBuffer>();
  const scopeEmitters = new Map<number, Emitter<VSBuffer>>();
  let unknownScopeFrames = 0;
  let invalidFrames = 0;
  let disposed = false;

  const subscription = transport.onMessage((buffer) => {
    if (disposed) {
      return;
    }
    const decoded = decodeFrame(buffer.buffer);
    if (!decoded) {
      invalidFrames += 1;
      options.onInvalidFrame?.();
      return;
    }
    if (decoded.scopeId === null) {
      baseEmitter.fire(VSBuffer.wrap(decoded.payload));
      return;
    }
    const emitter = scopeEmitters.get(decoded.scopeId);
    if (emitter) {
      emitter.fire(VSBuffer.wrap(decoded.payload));
      return;
    }
    unknownScopeFrames += 1;
    options.onUnknownScope?.(decoded.scopeId);
  });

  const base: IMessagePassingProtocol = {
    send: (buffer) => transport.send(buffer),
    onMessage: baseEmitter.event,
  };

  return {
    base,
    attachScope(scopeId: number): IMessagePassingProtocol {
      assertScopeId(scopeId);
      if (scopeEmitters.has(scopeId)) {
        throw new Error(`scope ${scopeId} 已经绑定`);
      }
      const emitter = new Emitter<VSBuffer>();
      scopeEmitters.set(scopeId, emitter);
      return {
        send: (buffer) => transport.send(VSBuffer.wrap(encodeScopedFrame(scopeId, buffer.buffer))),
        onMessage: emitter.event,
      };
    },
    detachScope(scopeId: number): void {
      const emitter = scopeEmitters.get(scopeId);
      scopeEmitters.delete(scopeId);
      emitter?.dispose();
    },
    get unknownScopeFrames() {
      return unknownScopeFrames;
    },
    get invalidFrames() {
      return invalidFrames;
    },
    dispose(): void {
      if (disposed) {
        return;
      }
      disposed = true;
      subscription.dispose();
      baseEmitter.dispose();
      for (const emitter of scopeEmitters.values()) {
        emitter.dispose();
      }
      scopeEmitters.clear();
    },
  };
}
