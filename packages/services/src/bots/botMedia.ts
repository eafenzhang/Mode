import { resolve as resolvePath, sep } from "node:path";

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
