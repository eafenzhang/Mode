/**
 * IM SDK 运行时包的 tar.gz 解包器。
 *
 * 只接受普通文件与目录条目；符号链接、硬链接、设备文件一律拒绝。路径、条目数、
 * 总大小都有上限，gzip 输出同样限长（防压缩炸弹）。支持 ustar 的 prefix 字段与
 * pax/GNU 长路径头，便于发布侧用常见 `tar` 直接打包。
 */
import { gunzipSync } from "node:zlib";

import { ImSdkRuntimeError, isSafeBundleRelativePath } from "./imSdkManifest.js";

export const IM_SDK_TAR_MAX_FILES = 20_000;
export const IM_SDK_TAR_MAX_BYTES = 512 * 1024 * 1024;

const BLOCK_SIZE = 512;

export interface ExtractImSdkRuntimeTarGzOptions {
  maxBytes?: number;
  maxFiles?: number;
}

function unsafe(message: string): ImSdkRuntimeError {
  return new ImSdkRuntimeError("unsafe-archive", message);
}

export function extractImSdkRuntimeTarGz(
  archive: Buffer,
  options: ExtractImSdkRuntimeTarGzOptions = {},
): Map<string, Buffer> {
  const maxBytes = options.maxBytes ?? IM_SDK_TAR_MAX_BYTES;
  const maxFiles = options.maxFiles ?? IM_SDK_TAR_MAX_FILES;
  let tar: Buffer;
  try {
    // 头块与补齐也占输出空间：上限按「文件字节 + 每文件 2 KiB」放宽，文件总量另算。
    tar = gunzipSync(archive, { maxOutputLength: maxBytes + maxFiles * 2048 + 8192 });
  } catch (error) {
    throw unsafe(`cannot gunzip bundle: ${error instanceof Error ? error.message : String(error)}`);
  }

  const files = new Map<string, Buffer>();
  let totalBytes = 0;
  let offset = 0;
  let pendingPath: string | undefined;
  let pendingSize: number | undefined;

  while (offset + BLOCK_SIZE <= tar.length) {
    const header = tar.subarray(offset, offset + BLOCK_SIZE);
    if (isZeroBlock(header)) break;
    verifyHeaderChecksum(header);

    const type = String.fromCharCode(header[156] ?? 0);
    // pax 的 size 只作用于紧随其后的真实条目；扩展头自身仍用头块里的尺寸，
    // 否则连续两个 pax 头会把前一条的 size 误用到后一条扩展头上。
    const isExtendedHeader = type === "x" || type === "L" || type === "g";
    let size = readOctal(header, 124, 12);
    if (!isExtendedHeader) {
      size = pendingSize ?? size;
      pendingSize = undefined;
    }
    if (size > maxBytes || !Number.isSafeInteger(size)) {
      throw unsafe(`declared size ${size} exceeds the bundle limit`);
    }
    const dataStart = offset + BLOCK_SIZE;
    const dataEnd = dataStart + size;
    if (dataEnd > tar.length) throw unsafe("archive is truncated");
    const payload = tar.subarray(dataStart, dataEnd);
    offset = dataStart + Math.ceil(size / BLOCK_SIZE) * BLOCK_SIZE;

    if (type === "x") {
      const pax = parsePaxRecords(payload);
      if (pax.path !== undefined) pendingPath = pax.path;
      if (pax.size !== undefined) pendingSize = pax.size;
      continue;
    }
    if (type === "L") {
      pendingPath = trimTrailingNuls(payload.toString("utf8"));
      continue;
    }
    if (type === "g") {
      // 全局 pax 只影响扩展属性，不参与路径/大小解析。
      continue;
    }

    const path = pendingPath ?? readEntryPath(header);
    pendingPath = undefined;
    if (type === "5") {
      // 空目录不落盘：安装时由文件路径按需创建。
      continue;
    }
    if (type !== "0" && type !== "\0") {
      throw unsafe(`unsupported tar entry type "${type}" for ${path || "<unnamed>"}`);
    }
    if (!isSafeBundleRelativePath(path)) {
      throw unsafe(`unsafe path in archive: ${JSON.stringify(path)}`);
    }
    if (files.has(path)) throw unsafe(`duplicate path in archive: ${path}`);
    if (files.size + 1 > maxFiles) throw unsafe(`archive has more than ${maxFiles} files`);
    totalBytes += payload.length;
    if (totalBytes > maxBytes) throw unsafe(`archive expands beyond ${maxBytes} bytes`);
    files.set(path, payload);
  }

  if (files.size === 0) throw unsafe("archive contains no files");
  return files;
}

function isZeroBlock(block: Buffer): boolean {
  for (const byte of block) {
    if (byte !== 0) return false;
  }
  return true;
}

function trimTrailingNuls(value: string): string {
  let end = value.length;
  while (end > 0 && value.charCodeAt(end - 1) === 0) end -= 1;
  return value.slice(0, end);
}

/**
 * ustar 校验和：累加整个头块，校验和字段本身按空格计。
 * 只接受八进制写法；校验和不匹配说明头块损坏。
 */
function verifyHeaderChecksum(header: Buffer): void {
  const expected = readOctal(header, 148, 8);
  let actual = 0;
  for (let index = 0; index < BLOCK_SIZE; index += 1) {
    const inChecksumField = index >= 148 && index < 156;
    actual += inChecksumField ? 0x20 : (header[index] ?? 0);
  }
  if (expected !== actual) {
    throw unsafe(`tar header checksum mismatch (expected ${expected}, got ${actual})`);
  }
}

function readOctal(header: Buffer, offset: number, length: number): number {
  const raw = header.subarray(offset, offset + length);
  let end = 0;
  while (end < raw.length && raw[end] !== 0 && raw[end] !== 0x20) end += 1;
  const text = raw.subarray(0, end).toString("ascii").trim();
  if (text === "") return 0;
  if (!/^[0-7]+$/.test(text)) throw unsafe(`invalid octal field at offset ${offset}`);
  const value = Number.parseInt(text, 8);
  if (!Number.isSafeInteger(value)) throw unsafe(`octal field at offset ${offset} is too large`);
  return value;
}

function readCString(header: Buffer, offset: number, length: number): string {
  const raw = header.subarray(offset, offset + length);
  const end = raw.indexOf(0);
  return raw.subarray(0, end < 0 ? raw.length : end).toString("utf8");
}

/** name（100）+ ustar prefix（155）：常见 `tar` 会把长路径拆进 prefix。 */
function readEntryPath(header: Buffer): string {
  const name = readCString(header, 0, 100);
  const prefix = readCString(header, 345, 155);
  return prefix ? `${prefix}/${name}` : name;
}

interface PaxRecords {
  path?: string;
  size?: number;
}

/**
 * pax 记录：`<长度> <key>=<value>\n`，长度包含自身与换行。
 * 只取 path/size；无法解析的记录直接忽略（不改写路径判定）。
 */
function parsePaxRecords(payload: Buffer): PaxRecords {
  const records: PaxRecords = {};
  let cursor = 0;
  while (cursor < payload.length) {
    const space = payload.indexOf(0x20, cursor);
    if (space < 0) break;
    const length = Number.parseInt(payload.subarray(cursor, space).toString("ascii"), 10);
    if (!Number.isSafeInteger(length) || length <= 0 || cursor + length > payload.length) {
      throw unsafe("malformed pax extended header");
    }
    const end = cursor + length;
    if (payload[end - 1] !== 0x0a) throw unsafe("malformed pax record terminator");
    const record = payload.subarray(space + 1, end - 1).toString("utf8");
    const equals = record.indexOf("=");
    if (equals > 0) {
      const key = record.slice(0, equals);
      const value = record.slice(equals + 1);
      if (key === "path") records.path = value;
      if (key === "size") {
        const parsed = Number(value);
        if (!Number.isSafeInteger(parsed) || parsed < 0) {
          throw unsafe("malformed pax size record");
        }
        records.size = parsed;
      }
    }
    cursor = end;
  }
  return records;
}
