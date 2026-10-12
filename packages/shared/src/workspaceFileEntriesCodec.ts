import type { WorkspaceFileEntry } from "@mode/shared";

/**
 * 工作区文件条目的列式编解码（worker/跨进程传输专用，零运行时依赖）。
 * 每行 `type\trelativePath`——name 是 relativePath 的最后段、path 由
 * rootPath 拼回，不重复传输（37 万条实测 65.7MB → ~25MB）。
 * Unix 文件名可含 \t、\n、\\，打包时转义、解包时还原。
 *
 * 传输形态选择（性能约束，见 docs/specs/remote-workspace-rpc-bulk-chunking.md）：
 * - RPC 顶层返回裸 string：走框架的 String 快速路径（长度前缀+原始字节），
 *   避免 Object 的 JSON.stringify/parse 对含 \t\n 大字符串的转义开销（实测 6-9s）；
 * - 单条消息不超过 WORKSPACE_FILE_ENTRIES_CHUNK_SIZE：远端工作区全部 RPC 共享
 *   一条单连接 FIFO 通道，大帧会把后续小帧挡在队列里（队头阻塞）。单块按
 *   attachmentChunkMaxBytes（512KiB）口径取 512K 字符——路径为主 ASCII，
 *   典型 ≈512KB 字节（CJK 路径最坏 ~1.5MB）：线占用 ≈ 512KB ÷ 带宽，环回实测
 *   4×4MB 批量下小 RPC p95 98ms，512K 块把该尾延迟收敛近一个数量级。
 *   分块拉取 + 块间让出事件循环后，主线程每块只处理一小块（~6ms），输入永不冻结。
 */
const FIELD_SEPARATOR = "\t";
const LINE_SEPARATOR = "\n";

/** 单块目标大小（字符数）；调用方按 totalLength 分块拉取。 */
export const WORKSPACE_FILE_ENTRIES_CHUNK_SIZE = 512 * 1024;

function escapeField(value: string): string {
  return value.replace(/\\/g, "\\\\").replace(/\t/g, "\\t").replace(/\n/g, "\\n");
}

// 正则一次性还原转义（比逐字符循环快数倍，37 万行下节省秒级 worker 时间）。
const UNESCAPE_PATTERN = /\\(.)/g;

function unescapeField(value: string): string {
  return value.replace(UNESCAPE_PATTERN, (match, char: string) => {
    if (char === "t") {
      return "\t";
    }
    if (char === "n") {
      return "\n";
    }
    return char;
  });
}

export function packWorkspaceFileEntries(entries: WorkspaceFileEntry[]): string {
  const lines: string[] = [];
  for (const entry of entries) {
    lines.push(`${entry.type}${FIELD_SEPARATOR}${escapeField(entry.relativePath)}`);
  }
  return lines.join(LINE_SEPARATOR);
}

/**
 * 解包并拼回完整 entries：name 取 relativePath 最后段，path 由 rootPath 拼出
 * （rootPath 以分隔符结尾与否都兼容；Windows 下用 \、其余用 /）。
 */
export function unpackWorkspaceFileEntries(packed: string, rootPath: string): WorkspaceFileEntry[] {
  if (packed.length === 0) {
    return [];
  }
  const separator = rootPath.includes("\\") ? "\\" : "/";
  const prefix =
    rootPath.endsWith("/") || rootPath.endsWith("\\") ? rootPath : `${rootPath}${separator}`;
  const lines = packed.split(LINE_SEPARATOR);
  const entries: WorkspaceFileEntry[] = [];
  for (const line of lines) {
    if (!line) {
      continue;
    }
    const tabAt = line.indexOf(FIELD_SEPARATOR);
    if (tabAt === -1) {
      continue;
    }
    const relativePath = unescapeField(line.slice(tabAt + 1));
    const lastSlash = relativePath.lastIndexOf("/");
    // Windows root 下把 posix 相对路径的分隔符也换回 \，保持与 node:path.join 一致。
    const pathPart = separator === "/" ? relativePath : relativePath.split("/").join("\\");
    entries.push({
      name: lastSlash === -1 ? relativePath : relativePath.slice(lastSlash + 1),
      path: `${prefix}${pathPart}`,
      relativePath,
      type: line.slice(0, tabAt) === "directory" ? "directory" : "file",
    });
  }
  return entries;
}
