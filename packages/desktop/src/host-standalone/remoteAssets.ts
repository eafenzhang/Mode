import { existsSync } from "node:fs";
import { join } from "node:path";

import { readExternalEnvVar } from "@mode/shared";
import { getAppConfigDir } from "@mode/services/node";

/** esbuild 构建期注入；开发态/测试态未声明时走 typeof 守卫。 */
declare const __MODE_REMOTE_ASSET_CDN_BASE_URL__: string | undefined;

/**
 * SSH/WSL/Docker 远程工作区部署所需的 remote assets 解析。
 *
 * 优先级与既有 Electron 线一致：显式 CDN 环境变量 > 打包资源目录 > 开发态
 * `packages/desktop/mock-cdn`。缓存目录固定在应用配置目录下，不写数据根。
 */
export interface RemoteAssetDirs {
  mockCdnDir?: string;
  remoteCdnBaseUrl?: string;
  remoteCdnBaseUrls?: string[];
  remoteCacheDir?: string;
}

function firstExistingDirectory(candidates: Array<string | undefined>): string | undefined {
  for (const candidate of candidates) {
    const trimmed = candidate?.trim();
    if (trimmed && existsSync(trimmed)) {
      return trimmed;
    }
  }
  return undefined;
}

export function resolveRemoteAssetDirs(env: NodeJS.ProcessEnv = process.env): RemoteAssetDirs {
  const resourcesDir = env.MODE_HOST_RESOURCES_DIR?.trim();
  const mockCdnDir = firstExistingDirectory([
    env.MODE_MOCK_CDN_DIR,
    resourcesDir ? join(resourcesDir, "mock-cdn") : undefined,
    join(process.cwd(), "packages", "desktop", "mock-cdn"),
  ]);
  const builtInCdnBaseUrl =
    typeof __MODE_REMOTE_ASSET_CDN_BASE_URL__ === "string"
      ? __MODE_REMOTE_ASSET_CDN_BASE_URL__
      : undefined;
  const remoteCdnBaseUrl =
    readExternalEnvVar(env, "MODE_REMOTE_ASSET_CDN_BASE_URL") ?? builtInCdnBaseUrl;
  const remoteCacheDir = env.MODE_REMOTE_ASSET_CACHE_DIR?.trim()
    ? env.MODE_REMOTE_ASSET_CACHE_DIR.trim()
    : join(getAppConfigDir(), "remote-assets-cache");
  return {
    ...(mockCdnDir ? { mockCdnDir } : {}),
    ...(remoteCdnBaseUrl ? { remoteCdnBaseUrl } : {}),
    remoteCacheDir,
  };
}
