import { invalid } from "./errors.js";

const BASE64URL = /^[A-Za-z0-9_-]*$/;

/** 规范无 padding base64url 编码。 */
export function encodeBase64Url(bytes: Uint8Array): string {
  return Buffer.from(bytes).toString("base64url");
}

/** 只接受规范无 padding base64url；拒绝填充、空白与非规范编码。 */
export function decodeBase64Url(text: string, label = "base64url"): Uint8Array {
  if (!BASE64URL.test(text) || text.length % 4 === 1) {
    invalid(`${label} 不是规范 base64url`);
  }
  const bytes = Buffer.from(text, "base64url");
  if (bytes.toString("base64url") !== text) {
    invalid(`${label} 不是规范的 base64url 编码`);
  }
  return bytes;
}
