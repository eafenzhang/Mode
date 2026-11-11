/** 浏览器 base64（标准字母表）→ 字节；HTTP 桥的 `bodyBase64` 用它解码。 */
export function decodeBase64(bodyBase64: string): Uint8Array {
  const binary = atob(bodyBase64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}
