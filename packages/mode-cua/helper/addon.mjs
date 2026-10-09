// MODE_CUA_HELPER_ADDON 装载与 AxError 风格错误转码：env 缺失/装载失败一律抛
// code:"internal" 的启动错误，由 entry 发 type:"error" 握手后非零退出。
import { createRequire } from "node:module";

import { HELPER_ADDON_ENV } from "../broker-server.js";

// AxError 风格：.code 供 errorResponseFromException 与握手 error.code 消费；
// message 带 "<code>:" 前缀，与 napi 侧 "<code>:<human>" 字符串口径一致（Task 8 的 parseAxError 可复用同一形状）。
export function axError(code, message) {
  const error = new Error(`${code}: ${message}`);
  error.code = code;
  return error;
}

export function loadAddon(env = process.env) {
  const addonPath = env[HELPER_ADDON_ENV];
  if (typeof addonPath !== "string" || !addonPath.trim()) {
    throw axError("internal", `${HELPER_ADDON_ENV} is not set`);
  }
  try {
    return createRequire(import.meta.url)(addonPath);
  } catch (error) {
    // 绝对路径属诊断必需（brief 明示允许）；错误文本不含任何凭据——连接凭据只有 socket 路径本身。
    throw axError(
      "internal",
      `failed to load addon at ${addonPath}: ${error?.message ?? String(error)}`,
    );
  }
}
