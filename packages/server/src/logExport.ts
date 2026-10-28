/**
 * Web/fpk 的日志导出（P1：docs/specs/fnos-feature-parity-and-update.md）。
 *
 * 与桌面 `exportLogs` 的包格式保持一致（`tauri-platform-commands.md` §1.1）：
 * 只选 Host 线 `host-*.log`（最新 7 份），旧线/非 Host 文件进 `omitted` manifest；
 * fpk 另把 `cmd/main` 的 `info.log` 作为额外文件带上，否则 NAS 上只有 stdout 重定向，
 * 没有 Host 文件日志可导。
 *
 * 读取按「先 stat 选文件、再只读尾部」进行：NAS 上单个日志可能远大于导出上限，
 * 不能为了挑选先整文件读进内存（否则一次导出就能吃掉几百 MB）。
 */
import { open, readdir, stat } from "node:fs/promises";
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

interface LogEntry {
  name: string;
  size: number;
  path: string;
}

function isHostLogName(name: string): boolean {
  const lower = name.toLowerCase();
  return lower.startsWith("host-") && lower.endsWith(".log");
}

/** 选择最新的 7 份 Host 日志；非 Host 的 `.log`（旧线/轮转件）进 omitted。 */
export function selectHostLogEntries<T extends { name: string; size: number }>(
  entries: readonly T[],
): { included: T[]; omitted: Array<{ name: string; size: number }> } {
  const host: T[] = [];
  const others: T[] = [];
  for (const entry of entries) {
    if (!entry.name.toLowerCase().endsWith(".log")) {
      continue;
    }
    (isHostLogName(entry.name) ? host : others).push(entry);
  }
  host.sort((left, right) => left.name.localeCompare(right.name));
  const splitAt = Math.max(0, host.length - 7);
  const omitted = [
    ...host.slice(0, splitAt).map((entry) => ({ name: entry.name, size: entry.size })),
    ...others.map((entry) => ({ name: entry.name, size: entry.size })),
  ];
  omitted.sort((left, right) => left.name.localeCompare(right.name));
  return { included: host.slice(splitAt), omitted };
}

/** 既有签名（内容已读出）保留给单元测试与调用方；内部导出走 `selectHostLogEntries`。 */
export function selectHostLogFiles(files: LogBundleFile[]): LogBundleSelection {
  const { included, omitted } = selectHostLogEntries(
    files.map((file) => ({ name: file.name, size: file.bytes.length })),
  );
  const byName = new Map(files.map((file) => [file.name, file]));
  return {
    included: included.map((entry) => byName.get(entry.name) as LogBundleFile),
    omitted,
    truncated: [],
  };
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

async function listLogEntries(dir: string): Promise<LogEntry[]> {
  let names: string[];
  try {
    names = await readdir(dir);
  } catch {
    return [];
  }
  const entries: LogEntry[] = [];
  for (const name of names) {
    if (!name.toLowerCase().endsWith(".log")) {
      continue;
    }
    const path = join(dir, name);
    try {
      const info = await stat(path);
      if (info.isFile()) {
        entries.push({ name, size: info.size, path });
      }
    } catch {
      // 单个文件读失败不阻塞整次导出（诊断导出）。
    }
  }
  return entries;
}

async function readFileTail(
  path: string,
  maxBytes: number,
): Promise<{ bytes: Buffer; totalBytes: number } | null> {
  try {
    const info = await stat(path);
    const start = Math.max(0, info.size - maxBytes);
    const length = info.size - start;
    const handle = await open(path, "r");
    try {
      const buffer = Buffer.alloc(length);
      await handle.read(buffer, 0, length, start);
      return { bytes: buffer, totalBytes: info.size };
    } finally {
      await handle.close();
    }
  } catch {
    return null;
  }
}

export async function createLogBundle(params: {
  logDir: string;
  extraFiles?: readonly string[];
  now?: Date;
  maxFileBytes?: number;
  maxTotalBytes?: number;
}): Promise<LogBundleResult | null> {
  const maxFileBytes = params.maxFileBytes ?? LOG_EXPORT_MAX_FILE_BYTES;
  const maxTotalBytes = params.maxTotalBytes ?? LOG_EXPORT_MAX_TOTAL_BYTES;
  const { included: hostIncluded, omitted } = selectHostLogEntries(
    await listLogEntries(params.logDir),
  );

  const extras: LogEntry[] = [];
  for (const path of params.extraFiles ?? []) {
    try {
      const info = await stat(path);
      if (info.isFile()) {
        extras.push({ name: basename(path), size: info.size, path });
      }
    } catch {
      // 额外文件不存在/不可读：跳过，不影响已有 Host 日志。
    }
  }
  extras.sort((left, right) => left.name.localeCompare(right.name));

  const included: LogBundleFile[] = [];
  const truncated: LogBundleSelection["truncated"] = [];
  let totalBytes = 0;
  for (const entry of [...hostIncluded, ...extras]) {
    if (totalBytes >= maxTotalBytes) {
      omitted.push({ name: entry.name, size: entry.size });
      continue;
    }
    const limit = Math.min(maxFileBytes, maxTotalBytes - totalBytes);
    const tail = await readFileTail(entry.path, limit);
    if (!tail) {
      continue;
    }
    if (tail.bytes.length < tail.totalBytes) {
      truncated.push({
        name: entry.name,
        includedBytes: tail.bytes.length,
        totalBytes: tail.totalBytes,
      });
    }
    included.push({ name: entry.name, bytes: tail.bytes });
    totalBytes += tail.bytes.length;
  }
  if (included.length === 0) {
    return null;
  }
  omitted.sort((left, right) => left.name.localeCompare(right.name));
  truncated.sort((left, right) => left.name.localeCompare(right.name));

  const now = params.now ?? new Date();
  return {
    fileName: defaultLogExportFileName(now),
    bundle: renderLogBundle(now.toISOString(), params.logDir, {
      included,
      omitted,
      truncated,
    }),
  };
}
