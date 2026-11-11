const BASE64URL = /^[A-Za-z0-9_-]*$/;

/** 浏览器可用的规范无 padding base64url；不依赖 Buffer。 */
export function encodeBase64Url(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/u, "");
}

export function decodeBase64Url(text: string, label = "base64url"): Uint8Array {
  if (!BASE64URL.test(text) || text.length % 4 === 1) {
    throw new Error(`${label} 不是规范 base64url`);
  }
  const padded = text.replace(/-/g, "+").replace(/_/g, "/");
  const binary = atob(padded + "=".repeat((4 - (padded.length % 4)) % 4));
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  // 拒绝非规范编码（例如多余 padding 或非最短表示）。
  if (encodeBase64Url(bytes) !== text) {
    throw new Error(`${label} 不是规范的 base64url 编码`);
  }
  return bytes;
}
