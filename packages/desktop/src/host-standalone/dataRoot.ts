/**
 * Host sidecar 的数据根解析（**纯函数，不碰文件系统**）。
 *
 * 口径与仓库既有实现**逐字一致**：`packages/services/src/paths.ts` 的
 * `getDataBaseDir()` / `getModeDataRootDir()`——
 *
 * ```
 * 数据根 = join(数据基目录, ".mode")
 * 数据基目录 = MODE_DATA_BASE_DIR ?? 家目录（HOME / USERPROFILE）
 * ```
 *
 * **`MODE_DATA_BASE_DIR` 是「基目录」，不是「数据根」**：它替换的是 `homedir()` 前缀，
 * 不是 `~/.mode` 这个根（`paths.ts` 的注释原文：“Set the base directory for app data
 * (replaces homedir() prefix)”；`packages/desktop/src/main/desktopRuntimeEnv.ts` 也是把
 * `dataBaseDir` 原样传给子进程）。把基目录当根用会让真服务把数据写进 `<base>/.mode/.mode`——
 * Task 1 不写盘所以看不出来，Task 2 一接真存储就会落错地方。2026-10-16 的复评把这条钉为
 * 产品路径缺陷，本文件是修复后的口径。
 *
 * 家目录取 `USERPROFILE`（Windows）→ `HOME`（POSIX / Git Bash）→ `os.homedir()`。
 * 测试与本地跑靠基目录 + 家目录两条覆写把 Host 关进临时目录，绝不碰用户真实 `~/.mode`。
 *
 * **Task 1 的 Host 不写盘**：本模块只做解析与上报，不创建任何目录。真正的存储（`node:sqlite`
 * 等）是 M3 Task 2 的范围，届时写入路径必须走这里解析出来的 `dataRoot`。
 */
import { homedir } from "node:os";
import { join } from "node:path";

/** `~/.mode` 这个命名空间的目录名（与 `packages/services/src/paths.ts` 的 `MODE_DATA_ROOT_DIR_NAME` 一致）。 */
export const MODE_DATA_ROOT_DIR_NAME = ".mode";

/**
 * 数据基目录的来源，便于对「隔离是否生效」做可断言的上报（不靠猜）。
 * `home-pointer`：默认根 `~/.mode/v2/setting.json` 里持久化的 `dataBaseDir` 指针
 * （用户改过数据目录后重启，壳不会传 env，Host 靠它回到自定义根；spec §23）。
 */
export type DataBaseDirSource = "MODE_DATA_BASE_DIR" | "home-pointer" | "home";

export interface DataRootResolution {
  /** 数据**基**目录（`MODE_DATA_BASE_DIR` 或家目录）。 */
  dataBaseDir: string;
  /** 数据根 = `{dataBaseDir}/.mode`。 */
  dataRoot: string;
  /** 派生基目录时用的家目录（`source === "home"` 才有意义）。 */
  homeDir: string;
  source: DataBaseDirSource;
}

/** 环境变量里的第一个非空值。 */
function firstNonEmpty(...values: Array<string | undefined>): string | undefined {
  for (const value of values) {
    const trimmed = value?.trim();
    if (trimmed) return trimmed;
  }
  return undefined;
}

/** 解析数据基目录/数据根。空字符串视同未设置（与 `paths.ts` 的 `?.trim() || null` 同口径）。 */
export function resolveDataRoot(env: NodeJS.ProcessEnv): DataRootResolution {
  const override = firstNonEmpty(env.MODE_DATA_BASE_DIR);
  const homeDir = firstNonEmpty(env.USERPROFILE, env.HOME) ?? homedir();
  const dataBaseDir = override ?? homeDir;
  return {
    dataBaseDir,
    dataRoot: join(dataBaseDir, MODE_DATA_ROOT_DIR_NAME),
    homeDir,
    source: override ? "MODE_DATA_BASE_DIR" : "home",
  };
}

/** 默认根的设置文件路径（`~/.mode/v2/setting.json`）——重启引导指针的读取位置。 */
export function defaultRootSettingsFile(homeDir: string): string {
  return join(homeDir, MODE_DATA_ROOT_DIR_NAME, "v2", "setting.json");
}

/** 从 setting.json 原文解析持久化的 `dataBaseDir` 指针；坏 JSON/缺键/空串一律视为无指针。 */
export function persistedDataBaseDirFromSettings(
  raw: string | null | undefined,
): string | undefined {
  if (!raw) {
    return undefined;
  }
  try {
    const parsed = JSON.parse(raw) as { dataBaseDir?: unknown };
    if (typeof parsed.dataBaseDir === "string" && parsed.dataBaseDir.trim()) {
      return parsed.dataBaseDir.trim();
    }
  } catch {
    // 用户手动编辑或半截 JSON：按无指针处理，回落默认根（与 settingService 的坏文件容忍同口径）。
  }
  return undefined;
}

/** 把解析结果改写到持久化指针指向的基目录（env 覆写优先级不变，指针只在无覆写时生效）。 */
export function withPersistedDataBaseDir(
  resolution: DataRootResolution,
  pointerBaseDir: string | undefined,
): DataRootResolution {
  if (!pointerBaseDir || resolution.source !== "home") {
    return resolution;
  }
  return {
    ...resolution,
    dataBaseDir: pointerBaseDir,
    dataRoot: join(pointerBaseDir, MODE_DATA_ROOT_DIR_NAME),
    source: "home-pointer",
  };
}
