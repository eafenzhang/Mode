/**
 * bot 附件缓存的保留期与清理（docs/specs/bot-attachment-cache-retention.md）。
 *
 * 为什么需要它：缓存文件由 buildAttachmentCachePath 生成、由 cacheResolvedAttachment 写入，
 * 之后**没有任何回收路径**；而且存储清理把 <configDir>/bot-attachments 归入 config 类，
 * config 的可清理性是 none，手动清理也不会碰它 → 只增不减。
 *
 * 三段结构：纯计划（planAttachmentCacheEviction，只回答"删哪些"）+ 目录枚举 + 执行删除。
 * 计划是纯函数，删除动作留在调用方，便于单测。
 */
import type { Dirent } from "node:fs";
import { lstat, readdir, rm, rmdir } from "node:fs/promises";
import { dirname, join } from "node:path";

/** R1：非受保护文件超过 7 天即可删。 */
export const BOT_ATTACHMENT_CACHE_MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000;

/** R2：非受保护文件合计超过 1 GiB 时按 mtime 由旧到新淘汰。 */
export const BOT_ATTACHMENT_CACHE_MAX_TOTAL_BYTES = 1024 * 1024 * 1024;

/** R4：同一进程内两次实际清理之间的最小间隔。 */
export const BOT_ATTACHMENT_CACHE_PRUNE_INTERVAL_MS = 60 * 60 * 1000;

export interface AttachmentCacheEntry {
  path: string;
  sizeBytes: number;
  mtimeMs: number;
}

export interface AttachmentCachePruneResult {
  deletedCount: number;
  freedBytes: number;
  failedCount: number;
}

/**
 * 路径比较用的规范形式：分隔符统一成 "/"，Windows 上再折叠大小写。
 *
 * 大小写折叠只在 win32 生效——受保护集合读自状态文件、待删集合来自目录枚举，两者本应同源，
 * 但 Windows 路径大小写不敏感，一旦出现大小写差异（用户改过盘符写法等），
 * 比较失败就意味着**删掉队列还在引用的文件**，代价见 R3：整条排队消息会一起丢。
 * POSIX 上保持大小写敏感，不能一并折叠。
 */
export function canonicalAttachmentCachePath(value: string): string {
  const normalized = value.trim().replace(/\\/gu, "/");
  return process.platform === "win32" ? normalized.toLowerCase() : normalized;
}

/**
 * 纯计划：返回应当删除的文件（不改文件系统）。
 *
 * 顺序有讲究——先按年龄挑，再把"留下的"里超出总量上限的最旧者挑出来。受保护文件
 * 两个阶段都不参与：既不被年龄删除，也不计入 R2 的配额（否则队列里的文件会把配额吃满，
 * 反而挤出别的新文件）。
 */
export function planAttachmentCacheEviction(params: {
  entries: readonly AttachmentCacheEntry[];
  protectedPaths: ReadonlySet<string>;
  now: number;
  maxAgeMs?: number;
  maxTotalBytes?: number;
}): AttachmentCacheEntry[] {
  const maxAgeMs = params.maxAgeMs ?? BOT_ATTACHMENT_CACHE_MAX_AGE_MS;
  const maxTotalBytes = params.maxTotalBytes ?? BOT_ATTACHMENT_CACHE_MAX_TOTAL_BYTES;
  const protectedCanonical = new Set(
    [...params.protectedPaths].map((path) => canonicalAttachmentCachePath(path)),
  );

  const evicted = new Set<AttachmentCacheEntry>();
  const kept: AttachmentCacheEntry[] = [];
  for (const entry of params.entries) {
    if (protectedCanonical.has(canonicalAttachmentCachePath(entry.path))) {
      continue;
    }
    if (params.now - entry.mtimeMs > maxAgeMs) {
      evicted.add(entry);
      continue;
    }
    kept.push(entry);
  }

  let keptBytes = kept.reduce((total, entry) => total + entry.sizeBytes, 0);
  if (keptBytes > maxTotalBytes) {
    // 最旧的先走：刚写入的文件最后才可能被淘汰，避免和正在进行中的回合抢文件。
    const oldestFirst = [...kept].sort((a, b) => a.mtimeMs - b.mtimeMs);
    for (const entry of oldestFirst) {
      if (keptBytes <= maxTotalBytes) {
        break;
      }
      evicted.add(entry);
      keptBytes -= entry.sizeBytes;
    }
  }
  return [...evicted];
}

/** 递归枚举缓存文件；目录不存在或枚举中消失的条目都按"没有"处理。 */
export async function collectAttachmentCacheEntries(
  rootDir: string,
): Promise<AttachmentCacheEntry[]> {
  const entries: AttachmentCacheEntry[] = [];
  const walk = async (dir: string): Promise<void> => {
    let dirents: Dirent[];
    try {
      dirents = await readdir(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const dirent of dirents) {
      const full = join(dir, dirent.name);
      if (dirent.isDirectory()) {
        await walk(full);
        continue;
      }
      // 只认普通文件：符号链接不跟（lstat 也不会让 isFile 为真），避免顺着链接删到缓存外面。
      if (!dirent.isFile()) {
        continue;
      }
      try {
        const stats = await lstat(full);
        entries.push({ path: full, sizeBytes: stats.size, mtimeMs: stats.mtimeMs });
      } catch {
        // 枚举期间被删掉，跳过即可
      }
    }
  };
  await walk(rootDir);
  return entries;
}

/** 自底向上回收变空的目录；根目录本身保留，写入方不必重新 mkdir。 */
async function removeEmptyParents(
  rootDir: string,
  deletedPaths: readonly string[],
): Promise<void> {
  const directories = new Set<string>();
  for (const path of deletedPaths) {
    let current = dirname(path);
    while (current !== rootDir && current.startsWith(rootDir)) {
      directories.add(current);
      current = dirname(current);
    }
  }
  const deepestFirst = [...directories].sort((a, b) => b.length - a.length);
  for (const directory of deepestFirst) {
    await rmdir(directory).catch(() => {});
  }
}

/**
 * 执行一次清理。删除失败只计数不抛：清理是后台维护，不能让它把入站链路带崩（R4）。
 * force 语义让"文件已不存在"算成功，因此多窗口并发清理不会互相报错。
 */
export async function pruneAttachmentCache(params: {
  rootDir: string;
  protectedPaths: ReadonlySet<string>;
  now?: number;
  maxAgeMs?: number;
  maxTotalBytes?: number;
}): Promise<AttachmentCachePruneResult> {
  const entries = await collectAttachmentCacheEntries(params.rootDir);
  const targets = planAttachmentCacheEviction({
    entries,
    protectedPaths: params.protectedPaths,
    now: params.now ?? Date.now(),
    ...(params.maxAgeMs !== undefined ? { maxAgeMs: params.maxAgeMs } : {}),
    ...(params.maxTotalBytes !== undefined ? { maxTotalBytes: params.maxTotalBytes } : {}),
  });
  const result: AttachmentCachePruneResult = { deletedCount: 0, freedBytes: 0, failedCount: 0 };
  const deleted: string[] = [];
  for (const target of targets) {
    try {
      await rm(target.path, { force: true });
      result.deletedCount += 1;
      result.freedBytes += target.sizeBytes;
      deleted.push(target.path);
    } catch {
      result.failedCount += 1;
    }
  }
  if (deleted.length > 0) {
    await removeEmptyParents(params.rootDir, deleted);
  }
  return result;
}
