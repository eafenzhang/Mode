/**
 * Web/fpk 的临时文本附件（对应桌面 `createTempTextAttachment`）。
 *
 * 长粘贴文本在 composer 里落成服务端临时文件，附件随后由 Host 的 fileService 读取。
 * 文件名/落盘语义与 Tauri 命令保持一致（crates/platform/src/commands/temp_attachment.rs）：
 * 只取最后一段文件名、替换 Windows 非法字符、每次调用唯一子目录。
 */
import { randomUUID } from "node:crypto";
import { mkdir, readdir, rm, stat, writeFile } from "node:fs/promises";
import { join } from "node:path";

export const TEMP_ATTACHMENT_DEFAULT_FILENAME = "attachment.txt";
export const TEMP_ATTACHMENT_MAX_BYTES = 5 * 1024 * 1024;
/** 服务端临时附件保留 7 天；任务早已结束，避免 NAS 数据盘无限增长。 */
export const TEMP_ATTACHMENT_RETENTION_MS = 7 * 24 * 60 * 60 * 1000;
/** 总量上限：超过后按最旧优先清理。 */
export const TEMP_ATTACHMENT_MAX_TOTAL_BYTES = 256 * 1024 * 1024;
export const TEMP_ATTACHMENT_SWEEP_INTERVAL_MS = 6 * 60 * 60 * 1000;

export interface TempTextAttachmentResult {
  filename: string;
  localPath: string;
  mimeType: "text/plain";
  sizeBytes: number;
}

export function sanitizeTempAttachmentFilename(raw: string | undefined): string {
  const candidate = (raw ?? "").split(/[\\/]/u).pop()?.trim() ?? "";
  if (!candidate || candidate === "." || candidate === "..") {
    return TEMP_ATTACHMENT_DEFAULT_FILENAME;
  }
  let sanitized = "";
  for (const char of candidate) {
    // 控制字符与 Windows 非法文件名字符统一替换，保证跨平台落盘一致。
    sanitized +=
      char.charCodeAt(0) < 0x20 || '<>:"/\\|?*'.includes(char) ? "_" : char;
  }
  const trimmed = sanitized.replace(/[ .]+$/u, "");
  return trimmed || TEMP_ATTACHMENT_DEFAULT_FILENAME;
}

export interface TempAttachmentSweepResult {
  removed: number;
  removedBytes: number;
  remainingBytes: number;
}

async function directorySize(path: string, depth = 0): Promise<number> {
  if (depth > 3) {
    return 0;
  }
  const entries = await readdir(path, { withFileTypes: true }).catch(() => []);
  let total = 0;
  for (const entry of entries) {
    const child = join(path, entry.name);
    if (entry.isDirectory()) {
      total += await directorySize(child, depth + 1);
      continue;
    }
    if (entry.isFile()) {
      total += (await stat(child).catch(() => null))?.size ?? 0;
    }
  }
  return total;
}

/**
 * 清理过期/超额临时附件；启动时与每 6 小时执行。
 * 只删根目录下的唯一子目录，不触碰根目录本身；IO 失败按跳过处理。
 */
export async function sweepTempAttachments(
  rootDir: string,
  options: {
    now?: number;
    retentionMs?: number;
    maxTotalBytes?: number;
  } = {},
): Promise<TempAttachmentSweepResult> {
  const now = options.now ?? Date.now();
  const retentionMs = options.retentionMs ?? TEMP_ATTACHMENT_RETENTION_MS;
  const maxTotalBytes = options.maxTotalBytes ?? TEMP_ATTACHMENT_MAX_TOTAL_BYTES;
  const entries = await readdir(rootDir, { withFileTypes: true }).catch(() => []);
  let removed = 0;
  let removedBytes = 0;
  const kept: Array<{ path: string; mtimeMs: number; bytes: number }> = [];
  for (const entry of entries) {
    if (!entry.isDirectory()) {
      continue;
    }
    const path = join(rootDir, entry.name);
    const info = await stat(path).catch(() => null);
    if (!info) {
      continue;
    }
    const bytes = await directorySize(path);
    if (now - info.mtimeMs > retentionMs) {
      await rm(path, { recursive: true, force: true }).catch(() => undefined);
      removed += 1;
      removedBytes += bytes;
      continue;
    }
    kept.push({ path, mtimeMs: info.mtimeMs, bytes });
  }
  let remainingBytes = kept.reduce((sum, item) => sum + item.bytes, 0);
  if (remainingBytes > maxTotalBytes) {
    kept.sort((left, right) => left.mtimeMs - right.mtimeMs);
    for (const item of kept) {
      if (remainingBytes <= maxTotalBytes) {
        break;
      }
      await rm(item.path, { recursive: true, force: true }).catch(() => undefined);
      remainingBytes -= item.bytes;
      removed += 1;
      removedBytes += item.bytes;
    }
  }
  return { removed, removedBytes, remainingBytes };
}

export async function createTempTextAttachment(
  rootDir: string,
  payload: { text: string; filename?: string },
): Promise<TempTextAttachmentResult> {
  const filename = sanitizeTempAttachmentFilename(payload.filename);
  const directory = join(rootDir, `${String(process.pid)}-${String(Date.now())}-${randomUUID()}`);
  await mkdir(directory, { recursive: true });
  const localPath = join(directory, filename);
  await writeFile(localPath, payload.text, "utf8");
  return {
    filename,
    localPath,
    mimeType: "text/plain",
    sizeBytes: Buffer.byteLength(payload.text, "utf8"),
  };
}
