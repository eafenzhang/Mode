// 落盘脱敏（spec §7）：轨迹与报告写盘前对字符串执行；比较也用脱敏后的值，
// 避免同一 secret 在不同轮次以不同随机值出现导致假差异。
export type RedactorOptions = {
  /** 用户 home 目录（Windows `USERPROFILE` / Unix `HOME`）。 */
  homeDir?: string;
  /** 隔离数据根；出现即 `<data-root>`。 */
  dataRoot?: string;
  /** case workspace；出现即 `<cwd>`。 */
  cwd?: string;
  /** 额外要抹掉的域名（`MODE_EVAL_REDACT_DOMAINS` 逗号分隔）。 */
  domains?: readonly string[];
};

export type Redactor = {
  redactString(value: string): string;
  redactValue(value: unknown): unknown;
};

const SECRET_VALUE_PATTERN = /\b((?:api[_-]?key|token|secret|password))(\s*[:=]\s*)("[^"]*"|'[^']*'|\S+)/gi;
const SK_KEY_PATTERN = /\bsk-[A-Za-z0-9_-]{6,}\b/g;
const BEARER_PATTERN = /\bBearer\s+[A-Za-z0-9._~+/=-]+\b/gi;
const LOOPBACK_URL_PATTERN = /\b(?:https?|wss?):\/\/(?:127\.0\.0\.1|localhost|\[::1\]):\d+/g;
const SECRET_KEY_NAME_PATTERN = /(?:api[_-]?key|token|secret|password|authorization)/i;

export function createRedactor(options: RedactorOptions = {}): Redactor {
  const replacements: Array<[string, string]> = [];
  if (options.dataRoot && options.dataRoot.length > 0) {
    replacements.push([options.dataRoot, "<data-root>"]);
  }
  if (options.cwd && options.cwd.length > 0) {
    replacements.push([options.cwd, "<cwd>"]);
  }
  if (options.homeDir && options.homeDir.length > 0) {
    replacements.push([options.homeDir, "<home>"]);
  }
  for (const domain of options.domains ?? []) {
    const trimmed = domain.trim();
    if (trimmed.length > 0) replacements.push([trimmed, "<redacted-domain>"]);
  }
  // 长路径在前，避免短前缀把长路径切坏。
  replacements.sort((left, right) => right[0].length - left[0].length);

  const redactString = (value: string): string => {
    let result = value;
    for (const [raw, token] of replacements) {
      result = result.replaceAll(raw, token);
    }
    result = result.replace(SK_KEY_PATTERN, "<redacted-key>");
    result = result.replace(BEARER_PATTERN, "Bearer <redacted-key>");
    result = result.replace(SECRET_VALUE_PATTERN, "$1$2<redacted>");
    result = result.replace(LOOPBACK_URL_PATTERN, "<loopback-url>");
    return result;
  };

  const redactValue = (value: unknown, key?: string): unknown => {
    if (typeof key === "string" && SECRET_KEY_NAME_PATTERN.test(key)) {
      if (typeof value === "string") return "<redacted>";
      return value;
    }
    if (typeof value === "string") return redactString(value);
    if (Array.isArray(value)) return value.map((item) => redactValue(item));
    if (value !== null && typeof value === "object") {
      return Object.fromEntries(
        Object.entries(value as Record<string, unknown>).map(([innerKey, inner]) => [
          innerKey,
          redactValue(inner, innerKey),
        ]),
      );
    }
    return value;
  };

  return { redactString, redactValue };
}
