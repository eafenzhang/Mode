import { realpath, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, isAbsolute, resolve as resolvePath, sep } from "node:path";
import { getAppConfigDir } from "../paths.js";

/**
 * 出站媒体的纯函数：类型判定与路径白名单比较。
 * 抽成独立模块便于单测——这两条规则是"agent 产物只能发到用户自己聊天"的安全边界。
 */

/** 出站媒体大小上限（服务层按平台能力统一取保守值）。 */
export const BOT_MEDIA_IMAGE_LIMIT_BYTES = 10 * 1024 * 1024;
export const BOT_MEDIA_FILE_LIMIT_BYTES = 50 * 1024 * 1024;

/** 扩展名 → 图片 MIME（命中即按图片内联发送，其余按文件）。 */
const BOT_MEDIA_IMAGE_MIME: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  webp: "image/webp",
  gif: "image/gif",
  bmp: "image/bmp",
};

/** 出站媒体：按扩展名判定类型与 MIME；未知扩展名按文件处理。 */
export function resolveOutboundMediaKind(filename: string): {
  kind: "image" | "file";
  mimeType: string;
} {
  const extension = filename.split(".").pop()?.toLowerCase() ?? "";
  const imageMime = BOT_MEDIA_IMAGE_MIME[extension];
  return imageMime
    ? { kind: "image", mimeType: imageMime }
    : { kind: "file", mimeType: "application/octet-stream" };
}

/** 路径归属判断：Windows 大小写不敏感；前缀比较必须带分隔符，避免 /foo 命中 /foobar。 */
export function isPathInside(root: string, candidate: string): boolean {
  const normalize = (value: string): string =>
    process.platform === "win32" ? value.toLowerCase() : value;
  const rootNorm = normalize(resolvePath(root));
  const candidateNorm = normalize(resolvePath(candidate));
  return candidateNorm === rootNorm || candidateNorm.startsWith(`${rootNorm}${sep}`);
}

/**
 * 出站/交接附件的路径校验：白名单 → 类型 → 大小上限（从 deliverBotMedia 原样抽出，
 * 错误文案逐字保留，既有测试可能断言）。
 * 允许的根目录：传入的工作区根、应用数据目录、系统临时目录（截图/导出常落在这里）。
 * 用 realpath 解析符号链接后再比较，避免通过链接绕出白名单。
 *
 * @param workspaceRoot 校验根（目标对话工作区，或交接场景的源对话工作区）；
 *   白名单集合固定为 该根 + app data + tmp，spec bot-cross-chat-handoff R4。
 * @param rawPath 已 trim 的原始路径（绝对路径直接用，相对路径按 workspaceRoot 解析）。
 */
export async function resolveValidatedMediaFile(
  workspaceRoot: string | undefined,
  rawPath: string,
): Promise<{
  real: string;
  filename: string;
  kind: "image" | "file";
  mimeType: string;
  sizeBytes: number;
}> {
  const candidate = isAbsolute(rawPath)
    ? rawPath
    : workspaceRoot
      ? resolvePath(workspaceRoot, rawPath)
      : null;
  if (!candidate) {
    throw new Error("No workspace context to resolve the file path.");
  }
  const real = await realpath(candidate).catch(() => null);
  if (!real) {
    throw new Error(`File not found: ${rawPath}`);
  }
  const allowedRoots = [workspaceRoot, getAppConfigDir(), tmpdir()].filter(
    (root): root is string => Boolean(root?.trim()),
  );
  // 根目录本身也可能是符号链接（macOS 的 /tmp→/private/tmp），realpath 后再比较。
  const resolvedRoots = await Promise.all(
    allowedRoots.map((root) => realpath(root).catch(() => resolvePath(root))),
  );
  if (!resolvedRoots.some((root) => isPathInside(root, real))) {
    throw new Error("Path is outside the allowed directories (workspace / app data / temp).");
  }
  const fileStat = await stat(real);
  if (!fileStat.isFile()) {
    throw new Error("The path is not a regular file.");
  }
  const filename = basename(real);
  const { kind, mimeType } = resolveOutboundMediaKind(filename);
  const limit = kind === "image" ? BOT_MEDIA_IMAGE_LIMIT_BYTES : BOT_MEDIA_FILE_LIMIT_BYTES;
  if (fileStat.size > limit) {
    throw new Error(
      `File is too large (${Math.round(fileStat.size / 1024 / 1024)}MB > ${Math.round(limit / 1024 / 1024)}MB).`,
    );
  }
  return { real, filename, kind, mimeType, sizeBytes: fileStat.size };
}
