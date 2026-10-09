// napi AxError 线格式转码：to_napi 抛 Error，message 形如 "<code>:<human message>"
// （format!("{}:{}", code, message)，冒号后无空格；addon.mjs axError 带空格——两种都要拆）。
// 码表与 crates/mode-cua-ax/src/error.rs 的 ALLOWED_CODES 同源清单
//（以 docs/specs/computer-use-windows-runtime.md 的 17 键为准），前缀不匹配一律归 internal——
// 绝不把任意文本当码外发给 SDK。

export const AX_ERROR_CODES = [
  "permission_denied",
  "not_authorized",
  "launch_failed",
  "invalid_request",
  "element_unavailable",
  "not_settable",
  "not_selectable",
  "action_unavailable",
  "foreground_required",
  "controller_busy",
  "broker_unavailable",
  "version_mismatch",
  "stale_socket",
  "timeout",
  "unimplemented",
  "method_not_found",
  "internal",
];
const AX_ERROR_CODE_SET = new Set(AX_ERROR_CODES);

// 只按第一个 ":" 切（message 内部可含冒号）；前缀不在 17 码表或无前缀 →
// code=internal 且 message 保留整段原文（前缀丢了就没法诊断现场）。
export function parseAxError(err) {
  const raw = err instanceof Error ? err.message : String(err);
  const at = raw.indexOf(":");
  if (at === -1) return { code: "internal", message: raw };
  const prefix = raw.slice(0, at).trim();
  if (!AX_ERROR_CODE_SET.has(prefix)) return { code: "internal", message: raw };
  return { code: prefix, message: raw.slice(at + 1).trim() };
}
