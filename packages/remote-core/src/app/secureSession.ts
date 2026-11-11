import { NoiseHandshake, NoiseTransport } from "../adapters/noise.js";
import type { IdentityKeyPair } from "../adapters/identity.js";
import {
  decodeBusinessFrame,
  encodeBusinessFrame,
  type BusinessFrame,
} from "../domain/businessFrames.js";
import { decodeBase64Url, encodeBase64Url } from "../domain/base64url.js";
import { RemoteError } from "../domain/errors.js";
import { FragmentReassembler, fragmentMessage, isFragmentPayload } from "../domain/fragments.js";
import {
  MAX_SECURE_MESSAGE_BYTES,
  SECURE_FRAGMENT_PAYLOAD_BYTES,
} from "../domain/limits.js";
import { buildPrologue, type ConnectionContext } from "../domain/prologue.js";

export type SecureSessionState = "handshaking" | "ready" | "closed";

export interface SecureSessionOptions {
  role: "host" | "client";
  localIdentity: IdentityKeyPair;
  peerIdentityKey: string;
  context: ConnectionContext;
  sendHandshake: (step: 1 | 2, data: Uint8Array) => void;
  onFrame: (frame: BusinessFrame) => void;
  onReady: () => void;
  onError: (error: RemoteError) => void;
  onClose: (reason: string) => void;
}

interface PendingOutbound {
  bytes: Uint8Array;
}

const MAX_PENDING_OUTBOUND = 64;

/**
 * 一条 connectionId 上的 Noise 会话：握手经 Control 转发，业务帧只走数据面。
 * 同一实例禁止复用；任何认证/分片错误都终止整条连接。
 */
export class SecureSession {
  private readonly options: SecureSessionOptions;
  private readonly handshake: NoiseHandshake;
  private transport: NoiseTransport | null = null;
  private readonly reassembler = new FragmentReassembler();
  private sendData: ((bytes: Uint8Array) => void) | null = null;
  private state: SecureSessionState = "handshaking";
  private messageId = 0;
  private readonly pending: PendingOutbound[] = [];

  constructor(options: SecureSessionOptions) {
    this.options = options;
    const initiator = options.role === "client";
    this.handshake = new NoiseHandshake({
      initiator,
      localIdentity: options.localIdentity,
      responderStaticKey: initiator
        ? options.peerIdentityKey
        : options.localIdentity.identityKey,
      expectedPeerStaticKey: initiator ? null : options.peerIdentityKey,
      prologue: buildPrologue(options.context),
    });
  }

  get currentState(): SecureSessionState {
    return this.state;
  }

  get context(): ConnectionContext {
    return this.options.context;
  }

  /** Initiator 在传输选择完成后调用，写出握手消息 1。 */
  start(): void {
    if (this.options.role !== "client") {
      throw new RemoteError("noise_state", "只有 Client 发起握手");
    }
    if (this.state !== "handshaking") throw new RemoteError("noise_state", "会话不在握手阶段");
    this.emitHandshake(1);
  }

  handleHandshake(step: 1 | 2, data: Uint8Array): void {
    if (this.state !== "handshaking") return;
    if (this.options.role === "client" && step !== 2) {
      throw new RemoteError("protocol", "Client 只接受握手 step 2");
    }
    if (this.options.role === "host" && step !== 1) {
      throw new RemoteError("protocol", "Host 只接受握手 step 1");
    }
    try {
      this.handshake.readMessage(data);
      if (this.options.role === "host") {
        this.emitHandshake(2);
      } else {
        this.finishHandshake();
      }
    } catch (error) {
      this.fail(error);
    }
  }

  attachData(send: (bytes: Uint8Array) => void): void {
    if (this.state === "closed") return;
    this.sendData = send;
    if (this.transport) this.markReady();
  }

  handleData(bytes: Uint8Array): void {
    if (this.state === "closed") return;
    if (!this.transport || !this.sendData) {
      this.fail(new RemoteError("data_before_ready", "数据面在安全通道就绪前收到数据"));
      return;
    }
    try {
      const plaintext = this.transport.receive(bytes);
      let frameBytes: Uint8Array;
      if (isFragmentPayload(plaintext)) {
        const message = this.reassembler.push(plaintext);
        if (!message) return;
        frameBytes = message.plaintext;
      } else {
        frameBytes = plaintext;
      }
      this.options.onFrame(decodeBusinessFrame(frameBytes));
    } catch (error) {
      this.fail(error);
    }
  }

  send(frame: BusinessFrame): void {
    if (this.state === "closed") throw new RemoteError("session_closed", "会话已关闭");
    const bytes = encodeBusinessFrame(frame);
    if (this.state === "ready") {
      this.sendEncrypted(bytes);
      return;
    }
    if (this.pending.length >= MAX_PENDING_OUTBOUND) {
      throw new RemoteError("backpressure", "握手阶段待发送消息超过上限");
    }
    this.pending.push({ bytes });
  }

  close(reason: string): void {
    if (this.state === "closed") return;
    this.state = "closed";
    this.pending.length = 0;
    this.reassembler.clear();
    this.options.onClose(reason);
  }

  fail(error: unknown): void {
    if (this.state === "closed") return;
    const normalized =
      error instanceof RemoteError
        ? error
        : new RemoteError("secure_channel_failed", "安全通道失败", { cause: error });
    this.state = "closed";
    this.pending.length = 0;
    this.reassembler.clear();
    this.options.onError(normalized);
    this.options.onClose(normalized.message);
  }

  private emitHandshake(step: 1 | 2): void {
    try {
      const message = this.handshake.writeMessage();
      this.options.sendHandshake(step, message);
      if (this.handshake.isComplete) this.finishHandshake();
    } catch (error) {
      this.fail(error);
    }
  }

  private finishHandshake(): void {
    if (this.transport) return;
    try {
      this.transport = new NoiseTransport(this.handshake.split());
    } catch (error) {
      this.fail(error);
      return;
    }
    if (this.sendData) this.markReady();
  }

  private markReady(): void {
    if (this.state !== "handshaking") return;
    this.state = "ready";
    const pending = this.pending.splice(0, this.pending.length);
    for (const entry of pending) this.sendEncrypted(entry.bytes);
    this.options.onReady();
  }

  private sendEncrypted(bytes: Uint8Array): void {
    if (!this.transport || !this.sendData) {
      throw new RemoteError("session_not_ready", "安全通道尚未就绪");
    }
    if (bytes.length > MAX_SECURE_MESSAGE_BYTES) {
      throw new RemoteError("message_too_large", "业务消息超过 4 MiB");
    }
    if (bytes.length > SECURE_FRAGMENT_PAYLOAD_BYTES) {
      const messageId = this.messageId;
      this.messageId = (this.messageId + 1) >>> 0;
      for (const fragment of fragmentMessage(messageId, bytes)) {
        this.sendData(this.transport.send(fragment));
      }
      return;
    }
    this.sendData(this.transport.send(bytes));
  }
}

/** 测试辅助：把某个 role 的握手数据转成 control frame 可用的 base64url。 */
export function handshakeDataToWire(data: Uint8Array): string {
  return encodeBase64Url(data);
}

export function handshakeDataFromWire(data: string): Uint8Array {
  return decodeBase64Url(data, "secure.handshake.data");
}
