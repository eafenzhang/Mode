/**
 * fpk 载荷的交换/回滚/结构检查。
 *
 * 单独成文件是为了把「会动线上文件系统」的代码收在一个可审阅的位置；manager 只负责
 * 状态机和下载。交换约定：
 * - 暂存目录固定在 `$TRIM_APPDEST/.update`，与目标同一文件系统，rename 不跨设备；
 * - 交换前写 `in-progress`，交换完成删除；`cmd/main` 启动失败时按 backup 回滚；
 * - `app.sock` 是运行中的网关 socket，永远不参与交换。
 */
import { access, mkdir, readFile, readdir, rename, rm, stat, writeFile } from "node:fs/promises";
import { constants } from "node:fs";
import { join } from "node:path";
import { parseFpkManifestVersion, parseStableVersion } from "./version.js";

/** 载荷里必须存在的入口/目录；缺一个就拒绝应用。 */
export const REQUIRED_PAYLOAD_PATHS = [
  "bin/mode.mjs",
  "web/index.html",
  "server",
  "agent",
  "node_modules",
  "runtime/bin/node",
  "proxy.mjs",
  "ui/config",
] as const;

export async function validatePayload(appDir: string): Promise<void> {
  for (const relativePath of REQUIRED_PAYLOAD_PATHS) {
    try {
      await access(join(appDir, relativePath), constants.F_OK);
    } catch {
      throw new Error(`invalid_payload:${relativePath}`);
    }
  }
}

export interface StagedPayloadVersions {
  /** fpk 外壳 `manifest` 的 version；读不到为 null。 */
  shell: string | null;
  /** 载荷 `package.json` 的 version；读不到或非正式版为 null。 */
  payload: string | null;
}

/**
 * 读取 fpk 外壳与载荷各自声明的版本。两者必须都与 release 的 latest.json 一致：
 * 外壳版本决定应用中心记录，载荷版本决定运行时 `MODE_VERSION`，任何一处不一致都说明
 * 「这个 fpk 不是给这个版本打的」，交换后会出现应用中心版本与实际运行版本错位。
 */
export async function readStagedPayloadVersions(
  outerDir: string,
  appDir: string,
): Promise<StagedPayloadVersions> {
  const manifest = await readFile(join(outerDir, "manifest"), "utf8").catch(() => null);
  const packageJson = await readFile(join(appDir, "package.json"), "utf8").catch(() => null);
  let payload: string | null = null;
  if (packageJson) {
    try {
      const raw = (JSON.parse(packageJson) as { version?: unknown }).version;
      payload = typeof raw === "string" ? parseStableVersion(raw) : null;
    } catch {
      payload = null;
    }
  }
  return {
    shell: manifest ? parseFpkManifestVersion(manifest) : null,
    payload,
  };
}

/** 版本不一致时抛错；错误信息保留三个值，便于对比是哪一层打错了。 */
export function assertStagedPayloadVersion(
  versions: StagedPayloadVersions,
  expectedVersion: string,
): void {
  if (versions.shell === expectedVersion && versions.payload === expectedVersion) {
    return;
  }
  throw new Error(
    `version_mismatch:shell=${versions.shell ?? "unknown"},payload=${
      versions.payload ?? "unknown"
    },expected=${expectedVersion}`,
  );
}

export async function applyStagedPayload(params: {
  appDest: string;
  stagingRoot: string;
}): Promise<void> {
  const { appDest, stagingRoot } = params;
  const appDir = join(stagingRoot, "app");
  const backupDir = join(stagingRoot, "backup");
  const marker = join(stagingRoot, "in-progress");

  await mkdir(backupDir, { recursive: true });
  await writeFile(marker, "update", "utf8");
  try {
    const stagedEntries = await readdir(appDir);
    const stagedSet = new Set(stagedEntries);
    const currentEntries = await readdir(appDest);
    for (const entry of stagedEntries) {
      if (entry === "app.sock" || entry === ".update") {
        continue;
      }
      const currentPath = join(appDest, entry);
      const stagedPath = join(appDir, entry);
      const backupPath = join(backupDir, entry);
      if (await exists(currentPath)) {
        await rename(currentPath, backupPath);
      }
      await rename(stagedPath, currentPath);
    }
    // 旧载荷里存在、新载荷里没有的顶层项（例如已下线的 dev/）也要进 backup，
    // 否则升级后会留下跨版本混合目录；回滚时由 backup 原样恢复。
    for (const entry of currentEntries) {
      if (entry === "app.sock" || entry === ".update" || stagedSet.has(entry)) {
        continue;
      }
      await rename(join(appDest, entry), join(backupDir, entry));
    }

    // 交换完成：先删标记，再清理 staged app。backup 留给 cmd/main 在启动成功后清理。
    await rm(marker, { force: true });
    await rm(appDir, { recursive: true, force: true });
  } catch (error) {
    await rollbackStagedPayload({ appDest, stagingRoot }).catch(() => undefined);
    throw error;
  }
}

/** 按 backup 恢复；同时删掉只在新载荷里存在的顶层项，避免新旧混合。 */
export async function rollbackStagedPayload(params: {
  appDest: string;
  stagingRoot: string;
}): Promise<void> {
  const { appDest, stagingRoot } = params;
  const backupDir = join(stagingRoot, "backup");
  const marker = join(stagingRoot, "in-progress");
  const backupEntries = await readdir(backupDir).catch(() => []);
  const backupSet = new Set(backupEntries);
  const currentEntries = await readdir(appDest).catch(() => []);
  for (const entry of currentEntries) {
    if (entry === "app.sock" || entry === ".update" || backupSet.has(entry)) {
      continue;
    }
    await rm(join(appDest, entry), { recursive: true, force: true }).catch(() => undefined);
  }
  for (const entry of backupEntries) {
    const backupPath = join(backupDir, entry);
    const currentPath = join(appDest, entry);
    await rm(currentPath, { recursive: true, force: true }).catch(() => undefined);
    await rename(backupPath, currentPath).catch(() => undefined);
  }
  await rm(backupDir, { recursive: true, force: true }).catch(() => undefined);
  await rm(marker, { force: true }).catch(() => undefined);
}

async function exists(path: string): Promise<boolean> {
  try {
    await stat(path);
    return true;
  } catch {
    return false;
  }
}
