import type { BusinessFrame } from "../domain/businessFrames.js";
import { MAX_SECURE_MESSAGE_BYTES } from "../domain/limits.js";
import { decodeBase64Url, encodeBase64Url } from "./base64url.js";
import type { WebNoiseTransport } from "./noise.js";

/** 业务帧 → relay.ciphertext；超长直接拒绝，避免服务端二次裁剪。 */
export function encryptBusinessFrame(frame: BusinessFrame, transport: WebNoiseTransport): string {
  const bytes = new TextEncoder().encode(JSON.stringify(frame));
  if (bytes.length > MAX_SECURE_MESSAGE_BYTES) throw new Error("业务消息超过上限");
  return encodeBase64Url(transport.send(bytes));
}

/** relay.ciphertext → 业务帧；认证失败抛出，由调用方终止等待。 */
export function decryptBusinessFrame(
  ciphertext: string,
  transport: WebNoiseTransport,
): BusinessFrame {
  const plaintext = transport.receive(decodeBase64Url(ciphertext, "relay.ciphertext"));
  return JSON.parse(new TextDecoder().decode(plaintext)) as BusinessFrame;
}
