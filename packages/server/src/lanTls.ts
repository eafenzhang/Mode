import { readFileSync } from "node:fs";

/**
 * LAN 监听 TLS 配置：运维提供被客户端信任的 key/cert 文件。
 *
 * 两个文件必须同时给出；只给一个视为配置错误（fail loud），避免静默退回明文。
 * 自签名证书不在支持范围（客户端默认按系统 CA 校验）。
 */
export function resolveLanTls(
  env: NodeJS.ProcessEnv = process.env,
): { key: Buffer; cert: Buffer } | undefined {
  const keyFile = env.MODE_LAN_TLS_KEY_FILE?.trim();
  const certFile = env.MODE_LAN_TLS_CERT_FILE?.trim();
  if (!keyFile && !certFile) return undefined;
  if (!keyFile || !certFile) {
    throw new Error("MODE_LAN_TLS_KEY_FILE 与 MODE_LAN_TLS_CERT_FILE 必须同时提供");
  }
  return { key: readFileSync(keyFile), cert: readFileSync(certFile) };
}
