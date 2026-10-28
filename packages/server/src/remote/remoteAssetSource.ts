/**
 * 服务端 SSH/WSL/Docker 远程工作区的 remote assets 源解析
 * （docs/specs/web-remote-workspace-ssh-docker.md §3）。
 *
 * 与桌面 Host 同口径：显式 CDN 环境变量优先，其次本地 mock-cdn 目录；缓存固定在
 * 应用配置目录。两者都没有时必须在发起连接前拒绝，不能进入半截部署。
 */
import { existsSync } from "node:fs";
import { join } from "node:path";
import { readExternalEnvVar } from "@mode/shared";
import { getAppConfigDir } from "@mode/services/node";

export interface ServerRemoteAssetDirs {
  mockCdnDir?: string;
  remoteCdnBaseUrl?: string;
  remoteCacheDir?: string;
}

export const REMOTE_ASSET_SOURCE_REQUIRED_MESSAGE =
  "SSH/WSL/Docker 远程工作区缺少 remote assets 源：请设置 MODE_REMOTE_ASSET_CDN_BASE_URL，或提供本地 mock-cdn（MODE_MOCK_CDN_DIR）。";

function firstExistingDirectory(candidates: Array<string | undefined>): string | undefined {
  for (const candidate of candidates) {
    const trimmed = candidate?.trim();
    if (trimmed && existsSync(trimmed)) {
      return trimmed;
    }
  }
  return undefined;
}

export function resolveServerRemoteAssetDirs(
  env: NodeJS.ProcessEnv = process.env,
): ServerRemoteAssetDirs {
  const resourcesDir = env["MODE_HOST_RESOURCES_DIR"]?.trim();
  // 与桌面 Host 同优先级：显式目录 > 打包资源目录 > 仓库开发态 mock-cdn。
  const mockCdnDir = firstExistingDirectory([
    env["MODE_MOCK_CDN_DIR"],
    resourcesDir ? join(resourcesDir, "mock-cdn") : undefined,
    join(process.cwd(), "packages", "desktop", "mock-cdn"),
  ]);
  const remoteCdnBaseUrl = readExternalEnvVar(env, "MODE_REMOTE_ASSET_CDN_BASE_URL");
  const remoteCacheDir =
    env["MODE_REMOTE_ASSET_CACHE_DIR"]?.trim() ||
    join(getAppConfigDir(), "remote-assets-cache");
  return {
    ...(mockCdnDir ? { mockCdnDir } : {}),
    ...(remoteCdnBaseUrl ? { remoteCdnBaseUrl } : {}),
    remoteCacheDir,
  };
}

export function hasRemoteAssetSource(dirs: ServerRemoteAssetDirs): boolean {
  return Boolean(dirs.mockCdnDir || dirs.remoteCdnBaseUrl);
}
