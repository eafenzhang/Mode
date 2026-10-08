import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { basename, dirname, join } from "node:path";
import { createProjectId, type ProjectId } from "@mode/contracts";
import { resolveProjectMemoryRoot } from "@mode/core";

export function getCliStorageRoot(storageRoot: string): string {
  return basename(storageRoot) === "cli" ? storageRoot : join(storageRoot, "cli");
}

/**
 * 插件存储根。改名（ZCODIUM → Mode）后应用数据根从 <base>/.zcodium 变成 <base>/.mode，
 * 但插件目录（市场清单、登记表、已装插件与缓存）挂在 CLI 存储根下，历史位置是
 * <home>/.zcodium/cli/plugins（更早还有 <base>/.zcodium/cli/plugins）。直接换到一个空目录会
 * 让市场与已装插件看起来「消失」（安装时报 Marketplace not found for dependency）。
 * 因此：新根有市场内容就用新根；否则若任一历史根有内容，继续沿用历史根——
 * 不复制、不移动，零丢失。
 */
export function getPluginStorageRoot(cliStorageRoot: string): string {
  const root = join(cliStorageRoot, "plugins");
  if (existsSync(join(root, "marketplaces"))) return root;
  const legacyRoots = [
    join(dirname(cliStorageRoot), ".zcodium", "cli", "plugins"),
    join(homedir(), ".zcodium", "cli", "plugins"),
  ];
  for (const legacy of legacyRoots) {
    if (legacy !== root && existsSync(join(legacy, "marketplaces"))) return legacy;
  }
  return root;
}

export function getModelIoDir(cliStorageRoot: string, isDevelopment: boolean): string {
  return join(cliStorageRoot, isDevelopment ? "debug" : "rollout");
}

export function getProjectMemoryRoot(
  cliStorageRoot: string,
  workingDirectory: string,
  workspaceIdentity?: string,
): string {
  return resolveProjectMemoryRoot({
    cliStorageRoot,
    workspaceIdentity,
    workspacePath: workingDirectory,
  });
}

export function projectIdFromDirectory(directory: string): ProjectId {
  return createProjectId(
    directory
      .toLowerCase()
      .replace(/[^a-z0-9._-]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 80) || "default",
  );
}
