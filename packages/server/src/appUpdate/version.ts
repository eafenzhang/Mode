/**
 * fpk 在线升级的纯函数（docs/specs/fnos-feature-parity-and-update.md）。
 *
 * 只处理**正式版** `x.y.z`：fnOS 应用中心对点分预发布后缀比较不友好，升级链路也
 * 不承诺把用户从正式版带到预发布。预发布版本在这里被显式忽略，返回 null，
 * 调用方按「不是可升级版本」处理，而不是猜一个排序。
 */

const STABLE_VERSION_PATTERN = /^(\d+)\.(\d+)\.(\d+)$/u;

/** 去掉可选 `v` 前缀；非正式版返回 null。 */
export function parseStableVersion(value: string | undefined | null): string | null {
  if (typeof value !== "string") {
    return null;
  }
  const trimmed = value.trim().replace(/^v/iu, "");
  return STABLE_VERSION_PATTERN.test(trimmed) ? trimmed : null;
}

/** 只比较正式版；任一不是正式版时抛错（调用方必须先过滤）。 */
export function compareStableVersions(left: string, right: string): number {
  const a = parseStableVersion(left);
  const b = parseStableVersion(right);
  if (!a || !b) {
    throw new Error(`not a stable version: left=${left} right=${right}`);
  }
  const pa = a.split(".").map(Number);
  const pb = b.split(".").map(Number);
  for (let index = 0; index < 3; index += 1) {
    const diff = (pa[index] as number) - (pb[index] as number);
    if (diff !== 0) {
      return diff > 0 ? 1 : -1;
    }
  }
  return 0;
}

/** 从 `sha256.txt`（`<hash>  <file>` 每行）里取指定文件名的哈希。 */
export function parseSha256ForFile(manifest: string, fileName: string): string | null {
  for (const rawLine of manifest.split(/\r?\n/u)) {
    const line = rawLine.trim();
    if (!line) {
      continue;
    }
    const match = /^([0-9a-f]{64})\s+\*?(.+)$/iu.exec(line);
    if (!match) {
      continue;
    }
    if ((match[2] as string).trim() === fileName) {
      return (match[1] as string).toLowerCase();
    }
  }
  return null;
}

export function fpkAssetName(version: string): string {
  return `mode-${version}-x86_64.fpk`;
}

/** 从 fpk 外壳 `manifest`（`key = value` 每行）读取 `version`；非正式版/缺失返回 null。 */
export function parseFpkManifestVersion(manifest: string): string | null {
  for (const rawLine of manifest.split(/\r?\n/u)) {
    const match = /^\s*version\s*=\s*(.+?)\s*$/u.exec(rawLine);
    if (match) {
      return parseStableVersion(match[1]);
    }
  }
  return null;
}

export function buildReleaseUrls(
  baseUrl: string,
  version: string,
): { latestManifest: string; checksums: string; checksumsSignature: string; fpk: string } {
  const base = baseUrl.replace(/\/+$/u, "");
  return {
    latestManifest: `${base}/latest/download/latest.json`,
    checksums: `${base}/download/v${version}/sha256.txt`,
    // 可选：同一 release 的分离签名（Ed25519），只在配置了公钥时请求。
    checksumsSignature: `${base}/download/v${version}/sha256.txt.sig`,
    fpk: `${base}/download/v${version}/${fpkAssetName(version)}`,
  };
}
