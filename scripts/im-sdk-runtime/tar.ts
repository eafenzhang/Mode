/**
 * 构造 IM SDK 运行时升级包用的确定性 tar.gz。
 *
 * 只写普通文件条目（ustar 头 + 512 对齐 + 末尾两个零块），路径排序、mtime 归零、
 * 权限固定 0644，保证同一棵 node_modules 产出相同字节，便于清单哈希与人工复核。
 */
import { createHash } from "node:crypto";
import { gzipSync } from "node:zlib";

export interface TarEntry {
  /** 包内相对路径，POSIX 分隔符。 */
  path: string;
  data: Buffer;
}

const BLOCK = 512;

export function createTarGz(entries: readonly TarEntry[]): Buffer {
  const sorted = [...entries].sort((left, right) => (left.path < right.path ? -1 : 1));
  const chunks: Buffer[] = [];
  const seen = new Set<string>();
  for (const entry of sorted) {
    if (seen.has(entry.path)) throw new Error(`duplicate tar path: ${entry.path}`);
    seen.add(entry.path);
    chunks.push(createHeader(entry.path, entry.data.length));
    chunks.push(entry.data);
    const padding = (BLOCK - (entry.data.length % BLOCK)) % BLOCK;
    if (padding > 0) chunks.push(Buffer.alloc(padding));
  }
  chunks.push(Buffer.alloc(BLOCK * 2));
  return gzipSync(Buffer.concat(chunks), { level: 9 });
}

export function sha256Hex(bytes: Buffer): string {
  return createHash("sha256").update(bytes).digest("hex");
}

function createHeader(path: string, size: number): Buffer {
  const header = Buffer.alloc(BLOCK);
  const { name, prefix } = splitUstarPath(path);
  header.write(name, 0, 100, "utf8");
  writeOctal(header, 100, 8, 0o644); // mode
  writeOctal(header, 108, 8, 0); // uid
  writeOctal(header, 116, 8, 0); // gid
  writeOctal(header, 124, 12, size);
  writeOctal(header, 136, 12, 0); // mtime：固定值，保证可复现
  header.write("        ", 148, 8, "ascii"); // 校验和占位（8 个空格）
  header.write("0", 156, 1, "ascii"); // regular file
  header.write("ustar\0", 257, 6, "ascii");
  header.write("00", 263, 2, "ascii");
  if (prefix) header.write(prefix, 345, 155, "utf8");
  let checksum = 0;
  for (const byte of header) checksum += byte;
  header.write(`${checksum.toString(8).padStart(6, "0")}\0 `, 148, 8, "ascii");
  return header;
}

function writeOctal(header: Buffer, offset: number, length: number, value: number): void {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new Error(`tar field at ${offset} is out of range: ${value}`);
  }
  const text = value.toString(8);
  if (text.length > length - 1) {
    throw new Error(`tar field at ${offset} does not fit ${length} bytes: ${text}`);
  }
  header.write(`${text.padStart(length - 1, "0")}\0`, offset, length, "ascii");
}

/**
 * ustar 的 name 最多 100 字节、prefix 最多 155 字节；长路径在 `/` 处拆成两段。
 * 两段都放不下时直接失败——发布包不应依赖 GNU 长路径扩展。
 */
function splitUstarPath(path: string): { name: string; prefix: string } {
  if (Buffer.byteLength(path, "utf8") <= 100) return { name: path, prefix: "" };
  const segments = path.split("/");
  for (let index = segments.length - 1; index > 0; index -= 1) {
    const prefix = segments.slice(0, index).join("/");
    const name = segments.slice(index).join("/");
    if (Buffer.byteLength(prefix, "utf8") <= 155 && Buffer.byteLength(name, "utf8") <= 100) {
      return { name, prefix };
    }
  }
  throw new Error(`tar path is too long for ustar: ${path}`);
}
