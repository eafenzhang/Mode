/**
 * Web/fpk 的日志导出（P1：docs/specs/fnos-feature-parity-and-update.md）。
 *
 * 与桌面 `exportLogs` 的包格式保持一致（`tauri-platform-commands.md` §1.1）：
 * 只选 Host 线 `host-*.log`（最新 7 份），旧线/非 Host 文件进 `omitted` manifest；
 * fpk 另把 `cmd/main` 的 `info.log` 作为额外文件带上，否则 NAS 上只有 stdout 重定向，
 * 没有 Host 文件日志可导。
 */
import { readFile, readdir } from "node:fs/promises";
import { basename, join } from "node:path";

export interface LogBundleFile {
  name: string;
  bytes: Buffer;
}

export interface LogBundleSelection {
  included: LogBundleFile[];
  omitted: Array<{ name: string; size: number }>;
  /** 单文件超限时只取尾部；记录原始大小，避免静默截断。 */
  truncated: Array<{ name: string; includedBytes: number; totalBytes: number }>;
}

export interface LogBundleResult {
  fileName: string;
  bundle: string;
}

/** 单文件与整包上限：NAS 上 info.log 无界增长，默认导出必须可控。 */
export const LOG_EXPORT_MAX_FILE_BYTES = 5 * 1024 * 1024;
export const LOG_EXPORT_MAX_TOTAL_BYTES = 20 * 1024 * 1024;

function isHostLogName(name: string): boolean {
  const lower = name.toLowerCase();
  return lower.startsWith("host-") && lower.endsWith(".log");
}

export function selectHostLogFiles(files: LogBundleFile[]): LogBundleSelection {
  const host: LogBundleFile[] = [];
  const others: LogBundleFile[] = [];
  for (const file of files) {
    if (!file.name.toLowerCase().endsWith(".log")) {
      continue;
    }
    (isHostLogName(file.name) ? host : others).push(file);
  }
  host.sort((left, right) => left.name.localeCompare(right.name));
  const included = host.slice(Math.max(0, host.length - 7));
  const omitted = [
    ...host.slice(0, Math.max(0, host.length - 7)),
    ...others,
  ].map((file) => ({ name: file.name, size: file.bytes.length }));
  omitted.sort((left, right) => left.name.localeCompare(right.name));
  return { included, omitted, truncated: [] };
}

export function renderLogBundle(
  exportedAt: string,
  logDir: string,
  selection: LogBundleSelection,
): string {
  const lines: string[] = [
    "Mode logs export",
    `exportedAt: ${exportedAt}`,
    `logDir: ${logDir}`,
    `files: ${String(selection.included.length)}`,
    `omitted: ${String(selection.omitted.length)}`,
    `truncated: ${String(selection.truncated.length)}`,
  ];
  for (const file of selection.omitted) {
    lines.push(`omittedFile: ${file.name} (${String(file.size)} bytes)`);
  }
  for (const file of selection.truncated) {
    lines.push(
      `truncatedFile: ${file.name} (included ${String(file.includedBytes)} of ${String(file.totalBytes)} bytes, tail)`,
    );
  }
  for (const file of selection.included) {
    lines.push("", `===== ${file.name} (${String(file.bytes.length)} bytes) =====`);
    lines.push(file.bytes.toString("utf8"));
  }
  let bundle = lines.join("\n");
  if (!bundle.endsWith("\n")) {
    bundle += "\n";
  }
  return bundle;
}

export function defaultLogExportFileName(now = new Date()): string {
  const stamp = now.toISOString().replace(/[-:]/gu, "").replace(/\..*$/u, "").replace("T", "-");
  return `Mode-logs-${stamp}.log`;
}

async function readLogFiles(dir: string): Promise<LogBundleFile[]> {
  let names: string[];
  try {
    names = await readdir(dir);
  } catch {
    return [];
  }
  const files: LogBundleFile[] = [];
  for (const name of names) {
    if (!name.toLowerCase().endsWith(".log")) {
      continue;
    }
    try {
      files.push({ name, bytes: await readFile(join(dir, name)) });
    } catch {
      // 单个文件读失败不阻塞整次导出（诊断导出）。
    }
  }
  files.sort((left, right) => left.name.localeCompare(right.name));
  return files;
}

export async function createLogBundle(params: {
  logDir: string;
  extraFiles?: readonly string[];
  now?: Date;
  maxFileBytes?: number;
  maxTotalBytes?: number;
}): Promise<LogBundleResult | null> {
  const files = await readLogFiles(params.logDir);
  const selection = selectHostLogFiles(files);

  const extras: LogBundleFile[] = [];
  for (const path of params.extraFiles ?? []) {
    try {
      extras.push({ name: basename(path), bytes: await readFile(path) });
    } catch {
      // 额外文件不存在/不可读：跳过，不影响已有 Host 日志。
    }
  }
  extras.sort((left, right) => left.name.localeCompare(right.name));
  selection.included = [...selection.included, ...extras];
  if (selection.included.length === 0) {
    return null;
  }

  const maxFileBytes = params.maxFileBytes ?? LOG_EXPORT_MAX_FILE_BYTES;
  const maxTotalBytes = params.maxTotalBytes ?? LOG_EXPORT_MAX_TOTAL_BYTES;
  const capped: LogBundleFile[] = [];
  let totalBytes = 0;
  for (const file of selection.included) {
    if (totalBytes >= maxTotalBytes) {
      selection.omitted.push({ name: file.name, size: file.bytes.length });
      continue;
    }
    const limit = Math.min(maxFileBytes, maxTotalBytes - totalBytes);
    const bytes =
      file.bytes.length > limit ? file.bytes.subarray(file.bytes.length - limit) : file.bytes;
    if (bytes.length < file.bytes.length) {
      selection.truncated.push({
        name: file.name,
        includedBytes: bytes.length,
        totalBytes: file.bytes.length,
      });
    }
    capped.push({ name: file.name, bytes });
    totalBytes += bytes.length;
  }
  selection.included = capped;
  selection.omitted.sort((left, right) => left.name.localeCompare(right.name));
  selection.truncated.sort((left, right) => left.name.localeCompare(right.name));

  const now = params.now ?? new Date();
  return {
    fileName: defaultLogExportFileName(now),
    bundle: renderLogBundle(now.toISOString(), params.logDir, selection),
  };
}
