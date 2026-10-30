/**
 * Mode Cloud / 中继的部署配置（docs/specs/mode-cloud-account-directory.md）。
 *
 * 目标：把「云端地址 + enrollment secret + relay token」作为**部署产物**一次写入，
 * 之后每台安装好的 Desktop 不再逐机设置环境变量。读取顺序：
 *   1. MODE_CLOUD_CONFIG_FILE
 *   2. <MODE_HOST_RESOURCES_DIR>/cloud-deployment.json（打包资源）
 *   3. <appConfigDir>/cloud-deployment.json（安装/开发机）
 *   4. <cwd>/cloud-deployment.json
 *
 * 文件是 secret 载体：权限 0600，不能提交到仓库。缺失时回退到现有 env，最后才
 * 保持 LAN-only。
 */
import { readFileSync, statSync } from "node:fs";
import { join } from "node:path";

import { getAppConfigDir } from "@mode/services/node";

export interface CloudDeploymentConfig {
  cloudBaseUrl?: string;
  enrollToken?: string;
  relayToken?: string;
  /** GitHub OAuth App 的 public client_id（需启用 Device Flow）；
   *  docs/specs/cloud-personal-accounts.md。 */
  githubClientId?: string;
}

interface CachedDeploymentConfig {
  value: CloudDeploymentConfig;
  /** 命中的文件路径；null = 所有候选都不存在，下次调用重新扫描。 */
  source: string | null;
  mtimeMs: number;
  size: number;
}

let cached: CachedDeploymentConfig | null = null;

function pickString(record: Record<string, unknown>, key: string): string | undefined {
  const value = record[key];
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

function parseConfig(value: unknown): CloudDeploymentConfig | null {
  if (!value || typeof value !== "object") {
    return null;
  }
  const record = value as Record<string, unknown>;
  const config: CloudDeploymentConfig = {
    ...(pickString(record, "cloudBaseUrl") ? { cloudBaseUrl: pickString(record, "cloudBaseUrl") } : {}),
    ...(pickString(record, "enrollToken") ? { enrollToken: pickString(record, "enrollToken") } : {}),
    ...(pickString(record, "relayToken") ? { relayToken: pickString(record, "relayToken") } : {}),
    ...(pickString(record, "githubClientId") ? { githubClientId: pickString(record, "githubClientId") } : {}),
  };
  return Object.keys(config).length > 0 ? config : null;
}

function configCandidates(
  env: NodeJS.ProcessEnv,
  cwd: string,
): string[] {
  const candidates: string[] = [];
  if (env.MODE_CLOUD_CONFIG_FILE?.trim()) {
    candidates.push(env.MODE_CLOUD_CONFIG_FILE.trim());
  }
  if (env.MODE_HOST_RESOURCES_DIR?.trim()) {
    candidates.push(join(env.MODE_HOST_RESOURCES_DIR.trim(), "cloud-deployment.json"));
  }
  try {
    candidates.push(join(getAppConfigDir(), "cloud-deployment.json"));
  } catch {
    // 数据根尚未初始化时忽略；环境变量/资源目录仍可生效。
  }
  candidates.push(join(cwd, "cloud-deployment.json"));
  return candidates;
}

export function loadCloudDeploymentConfig(
  env: NodeJS.ProcessEnv = process.env,
  cwd: string = process.cwd(),
): CloudDeploymentConfig {
  // 缓存按命中文件的 mtime + size 失效：部署文件在 Host 启动后才写入/更新时，
  // 不需要重启就能生效（否则 loadCloudDeploymentConfig 的空结果会锁死整个进程）。
  if (cached?.source) {
    try {
      const stat = statSync(cached.source);
      if (stat.mtimeMs === cached.mtimeMs && stat.size === cached.size) {
        return cached.value;
      }
    } catch {
      // 命中文件被删除/不可读：继续重新扫描全部候选。
    }
  }
  for (const candidate of configCandidates(env, cwd)) {
    try {
      const stat = statSync(candidate);
      if (!stat.isFile()) {
        continue;
      }
      const parsed = parseConfig(JSON.parse(readFileSync(candidate, "utf8")));
      if (parsed) {
        cached = { value: parsed, source: candidate, mtimeMs: stat.mtimeMs, size: stat.size };
        return parsed;
      }
    } catch {
      // 文件不存在/不可读/格式错误：继续尝试下一个候选。
    }
  }
  // 没有命中任何文件：下次调用重新扫描，配置后放不必重启。
  cached = { value: {}, source: null, mtimeMs: 0, size: 0 };
  return cached.value;
}

/** 测试用：清空缓存，允许在不同临时目录间切换。 */
export function resetCloudDeploymentConfigCacheForTests(): void {
  cached = null;
}
