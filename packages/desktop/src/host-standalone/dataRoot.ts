/**
 * Host sidecar 的数据根解析（**纯函数，不碰文件系统**）。
 *
 * 口径与 M2 的对拍口径一致：`MODE_DATA_BASE_DIR` 优先，否则由「家目录」派生 `<home>/.mode`；
 * 家目录取 `USERPROFILE`（Windows）→ `HOME`（POSIX / Git Bash）→ `os.homedir()`。
 * 测试与本地跑靠这两条覆写把 Host 关进临时目录，绝不碰用户真实的 `~/.mode`。
 *
 * **Task 1 的 Host 不写盘**：本模块只做解析与上报，不创建任何目录。真正的存储（`node:sqlite`
 * 等）是 M3 Task 2 的范围，届时写入路径也必须走这里解析出来的根。
 */
import { homedir } from "node:os";
import { join } from "node:path";

/** 数据根的来源，便于对「隔离是否生效」做可断言的上报（不靠猜）。 */
export type DataRootSource = "MODE_DATA_BASE_DIR" | "home";

export interface DataRootResolution {
  /** Host 认到的数据根绝对路径。 */
  dataRoot: string;
  /** 派生数据根时用的家目录（`source === "home"` 才有意义）。 */
  homeDir: string;
  source: DataRootSource;
}

/** 环境变量里的第一个非空值。 */
function firstNonEmpty(...values: Array<string | undefined>): string | undefined {
  for (const value of values) {
    const trimmed = value?.trim();
    if (trimmed) return trimmed;
  }
  return undefined;
}

export function resolveDataRoot(env: NodeJS.ProcessEnv): DataRootResolution {
  const override = firstNonEmpty(env.MODE_DATA_BASE_DIR);
  const homeDir = firstNonEmpty(env.USERPROFILE, env.HOME) ?? homedir();
  if (override) {
    return { dataRoot: override, homeDir, source: "MODE_DATA_BASE_DIR" };
  }
  return { dataRoot: join(homeDir, ".mode"), homeDir, source: "home" };
}
