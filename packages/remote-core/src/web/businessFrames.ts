import type { BusinessFrame } from "../domain/businessFrames.js";
import { MAX_SECURE_MESSAGE_BYTES } from "../domain/limits.js";
import { decodeBase64Url, encodeBase64Url } from "./base64url.js";
import { fragmentWebMessage, isWebFragment, WebFragmentReassembler } from "./fragments.js";
import type { WebNoiseTransport } from "./noise.js";

/** 业务帧 → UTF-8 明文；超长直接拒绝（由分片层负责切分，不是这里）。 */
export function encodeBusinessFrame(frame: BusinessFrame): Uint8Array {
  const bytes = new TextEncoder().encode(JSON.stringify(frame));
  if (bytes.length > MAX_SECURE_MESSAGE_BYTES) throw new Error("业务消息超过上限");
  return bytes;
}

export function decodeBusinessFrame(bytes: Uint8Array): BusinessFrame {
  return JSON.parse(new TextDecoder().decode(bytes)) as BusinessFrame;
}

/** 明文分片 → relay.ciphertext（一个 Noise 消息）。 */
export function encryptBusinessBytes(bytes: Uint8Array, transport: WebNoiseTransport): string {
  return encodeBase64Url(transport.send(bytes));
}

/** relay.ciphertext → 明文分片；认证失败抛出，由调用方终止等待。 */
export function decryptBusinessBytes(ciphertext: string, transport: WebNoiseTransport): Uint8Array {
  return transport.receive(decodeBase64Url(ciphertext, "relay.ciphertext"));
}

/** 浏览器业务数据面：明文分片 + Noise 加密 + 分片重组，一个连接一份。 */
export class WebBusinessCodec {
  private messageId = 0;
  private readonly reassembler = new WebFragmentReassembler();

  constructor(private readonly transport: WebNoiseTransport) {}

  /** 业务帧 → 多条 relay.ciphertext（每片一条 Noise 消息）。 */
  encode(frame: BusinessFrame): string[] {
    const bytes = encodeBusinessFrame(frame);
    if (bytes.length > MAX_SECURE_MESSAGE_BYTES) throw new Error("业务消息超过上限");
    const fragments = fragmentWebMessage(this.messageId, bytes);
    this.messageId = (this.messageId + 1) >>> 0;
    return fragments.map((fragment) => encryptBusinessBytes(fragment, this.transport));
  }

  /** relay.ciphertext → 业务帧；分片未集齐返回 null。 */
  decode(ciphertext: string): BusinessFrame | null {
    const plaintext = decryptBusinessBytes(ciphertext, this.transport);
    let message = plaintext;
    if (isWebFragment(plaintext)) {
      const complete = this.reassembler.push(plaintext);
      if (!complete) return null;
      message = complete.plaintext;
    }
    return decodeBusinessFrame(message);
  }
}
