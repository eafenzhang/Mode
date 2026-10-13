/**
 * 编码能力缓存的磁盘持久化（spec: native-media「编码能力缓存」2026-10-13，对标 UU 的
 * `encoder_codec_capability_cache.json`）：只把**真实会话观测到的**编码器可用性跨进程
 * 保留——设置页冷启动即可显示「未检测到硬件 H.264 编码器」提示，无需等第一次会话。
 *
 * 边界（保持 spec 口径，不扩权）：
 * - 不做 GPU 指纹/预探测（探测动作与首次会话共用同一路径，不为缓存额外拉起 helper）；
 * - GPU/驱动变更后缓存可能过期 → 下一次真实会话观测即覆盖（双向自愈）；
 * - 读取严格白名单（未知形状一律按无缓存），损坏文件不影响运行。
 */
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname } from "node:path";

export type EncoderCapability = "unknown" | "available" | "unavailable";

/** 缓存文件名（落在 app 配置目录；调用方拼完整路径）。 */
export const ENCODER_CAPABILITY_FILE = "rd-encoder-capability.json";

/** 白名单收敛：非对象/未知枚举值 → null（= 无缓存）。纯函数，单测覆盖。 */
export function sanitizeStoredEncoderCapability(raw: unknown): EncoderCapability | null {
  if (!raw || typeof raw !== "object") {
    return null;
  }
  const capability = (raw as { capability?: unknown }).capability;
  return capability === "available" || capability === "unavailable" || capability === "unknown"
    ? capability
    : null;
}

/** 读缓存；缺失/损坏/形状非法一律 null（下次真实会话观测后写回）。 */
export async function loadPersistedEncoderCapability(
  path: string,
): Promise<EncoderCapability | null> {
  try {
    return sanitizeStoredEncoderCapability(JSON.parse(await readFile(path, "utf8")));
  } catch {
    return null;
  }
}

/** 写缓存（fire-and-forget 由调用方负责；目录不存在时自建）。 */
export async function persistEncoderCapability(
  path: string,
  capability: EncoderCapability,
): Promise<void> {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, JSON.stringify({ capability, observedAt: Date.now() }), "utf8");
}
