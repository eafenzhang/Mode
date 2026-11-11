import { invalid } from "./errors.js";

export function asRecord(value: unknown, label: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    invalid(`${label} 必须是对象`);
  }
  return value as Record<string, unknown>;
}

/** 严格字段集合：未知字段一律拒绝，避免把未定义数据带进安全边界。 */
export function exactKeys(
  record: Record<string, unknown>,
  allowed: readonly string[],
  label: string,
): void {
  for (const key of Object.keys(record)) {
    if (!allowed.includes(key)) {
      invalid(`${label} 包含未知字段 ${key}`);
    }
  }
}

export function readString(
  record: Record<string, unknown>,
  key: string,
  label: string,
  options: { min?: number; max?: number; pattern?: RegExp; optional?: boolean } = {},
): string {
  const value = record[key];
  if (value === undefined && options.optional) return "";
  if (typeof value !== "string") invalid(`${label}.${key} 必须是字符串`);
  const min = options.min ?? 1;
  const max = options.max ?? 8192;
  const bytes = Buffer.byteLength(value, "utf8");
  if (bytes < min || bytes > max) {
    invalid(`${label}.${key} 长度必须在 ${min}..${max} 字节`);
  }
  if (options.pattern && !options.pattern.test(value)) {
    invalid(`${label}.${key} 格式非法`);
  }
  return value;
}

export function readInt(
  record: Record<string, unknown>,
  key: string,
  label: string,
  options: { min?: number; max?: number; optional?: boolean } = {},
): number {
  const value = record[key];
  if (value === undefined && options.optional) return 0;
  if (typeof value !== "number" || !Number.isSafeInteger(value)) {
    invalid(`${label}.${key} 必须是安全整数`);
  }
  const min = options.min ?? 0;
  const max = options.max ?? Number.MAX_SAFE_INTEGER;
  if (value < min || value > max) invalid(`${label}.${key} 超出范围`);
  return value;
}

export function readBoolean(
  record: Record<string, unknown>,
  key: string,
  label: string,
  optional = false,
): boolean {
  const value = record[key];
  if (value === undefined && optional) return false;
  if (typeof value !== "boolean") invalid(`${label}.${key} 必须是布尔值`);
  return value;
}

export function readArray(
  record: Record<string, unknown>,
  key: string,
  label: string,
  options: { min?: number; max?: number; optional?: boolean } = {},
): unknown[] {
  const value = record[key];
  if (value === undefined && options.optional) return [];
  if (!Array.isArray(value)) invalid(`${label}.${key} 必须是数组`);
  const min = options.min ?? 0;
  const max = options.max ?? 64;
  if (value.length < min || value.length > max) {
    invalid(`${label}.${key} 元素数量必须在 ${min}..${max}`);
  }
  return value;
}

export function readStringArray(
  record: Record<string, unknown>,
  key: string,
  label: string,
  options: { min?: number; max?: number; optional?: boolean } = {},
): string[] {
  const values = readArray(record, key, label, options);
  const result = values.map((entry, index) => {
    if (typeof entry !== "string" || entry.length < 1 || entry.length > 128) {
      invalid(`${label}.${key}[${index}] 必须是 1..128 字符字符串`);
    }
    return entry;
  });
  if (new Set(result).size !== result.length) {
    invalid(`${label}.${key} 不允许重复值`);
  }
  return result;
}

export function readObject(
  record: Record<string, unknown>,
  key: string,
  label: string,
  optional = false,
): Record<string, unknown> | undefined {
  const value = record[key];
  if (value === undefined && optional) return undefined;
  return asRecord(value, `${label}.${key}`);
}

export function utf8Length(value: string): number {
  return Buffer.byteLength(value, "utf8");
}
