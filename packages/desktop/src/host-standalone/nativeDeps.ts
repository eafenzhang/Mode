/**
 * Host sidecar 的**原生依赖探针**（M3 Task 2 · DR-T2-1）。
 *
 * 为什么它在生产代码里而不只在测试里：DR-M3-1 选 Node sidecar 的唯一理由是复用存量
 * `packages/services`，而那一层依赖 ABI 敏感的原生资产。探针如果只活在测试里，
 * 产品每次启动时「原生依赖到底好不好」就是不可观察的——而这是「判错就要重开 DR-M3-1」的量级，
 * 不该靠人记得去跑一次。所以：入口在启动时跑一次探针并把结果报进 `host.describe`，
 * 用例既断言探针本身、也断言 sidecar 进程内的这份报告（两层证据，见
 * `apps/desktop-tauri/test/nativeDeps.test.mts`）。
 *
 * ## 探什么、为什么这么探
 *
 * - `node:sqlite`：**真跑一次读写**（`:memory:` 建表 → 插入 → 查回 → 读 `sqlite_version()`）。
 *   「import 没抛错」证明不了 `DatabaseSync` 的实例化与语句执行可用，而真实存储层两件事都要。
 * - `node-pty`：默认档做**模块加载 + API 面（`spawn`/`fork`/`createTerminal`）+ 原生 artifact
 *   存在性**检查；`probePtySpawn` 才真起一个 pty 跑一条命令（`MODE_HOST_NATIVE_DEP_PROBE=spawn`
 *   时 sidecar 启动过程也会走这一档）。理由：Windows 上 node-pty **没有**任何「加载绑定」的
 *   公开入口（`exports.native` 在 win32 恒为 `null`，真加载只发生在 `spawn` 里），所以
 *   「不起进程」证不了绑定可用；而为了自检在每次产品启动时真起一个 shell 是有副作用的动作。
 *   两档分别把话说清楚，不用布尔占位掩盖差别。
 * - **CUA**：资产经 env 注入（`MODE_CUA_BUNDLED_HELPER_APP_PATH`，见
 *   `packages/services/src/node.ts` 的 `resolveBundledCuaHelperAppPath`）。CUA 是 opt-in 特性，
 *   **未注入不是失败**（缺省语义是「本机不提供 CUA」）；但**注入了不存在的路径是失败**——
 *   否则「配置写错」会被读成「CUA 正常但没启用」。
 *
 * 探针**非致命**：某一档失败只记账并上报，不让 Host 起不来。理由是一个原生依赖坏掉
 * 仍然能渲染大部分 UI 面，比「Host 直接不启动」对用户更有利；真实服务在使用该依赖时
 * 自己会抛（那才是它该失败的地方）。这条取舍写在这里，别在别处再兜一层。
 */
import { createRequire } from "node:module";
import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

/** `MODE_CUA_BUNDLED_HELPER_APP_PATH` 的字面量（真源在 `packages/services/src/node.ts`）。 */
export const MODE_CUA_BUNDLED_HELPER_APP_PATH_ENV = "MODE_CUA_BUNDLED_HELPER_APP_PATH";

/**
 * 深探测开关：设为 `spawn` 时，sidecar 启动过程会**真起一个 pty** 跑一条命令并把结果报进
 * `host.describe`。产品默认**不设**（起进程是有副作用的动作，不该为了自检在每次启动时发生）；
 * 测试显式设它，于是「在 sidecar 进程里真起得来 pty」也是一条被断言的事实，而不是推断。
 */
export const MODE_HOST_NATIVE_DEP_PROBE_ENV = "MODE_HOST_NATIVE_DEP_PROBE";

export interface SqliteProbeResult {
  ok: boolean;
  /** `select sqlite_version()` 的返回值（ok 时才有）。 */
  version?: string;
  /** 真读回一次的值（证明语句执行链路可用，不只是实例化）。 */
  roundTrip?: number;
  error?: string;
}

export interface PtyProbeResult {
  ok: boolean;
  /** 是否真起过一个 pty（`MODE_HOST_NATIVE_DEP_PROBE=spawn` 档才有）。 */
  spawned?: boolean;
  exitCode?: number;
  /** pty 收到的原始输出（真起进程档才有）。 */
  output?: string;
  /** 找到的原生 `.node` artifacts（相对 node-pty 包根）。加载档的核心证据。 */
  nativeArtifacts?: string[];
  /** 只做加载档时为 `false`：表示「模块与 API 面在、原生 artifact 在，但本轮没真起进程」。 */
  spawnVerified?: boolean;
  error?: string;
}

export interface CuaProbeResult {
  /** 环境是否注入了 CUA 资产路径。未注入是正常状态（opt-in 特性缺省关闭）。 */
  injected: boolean;
  /** 注入的路径是否存在；未注入时为 `false`（此时它不代表失败）。 */
  ok: boolean;
  path?: string;
  error?: string;
}

export interface NativeDepsReport {
  sqlite: SqliteProbeResult;
  pty: PtyProbeResult;
  cua: CuaProbeResult;
}

const require_ = createRequire(import.meta.url);

/** 只在 ok 时出现的字段：让报告在 JSON 里也不含糊（失败必带 `error`）。 */
function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function probeSqlite(): SqliteProbeResult {
  try {
    const { DatabaseSync } = require_("node:sqlite") as typeof import("node:sqlite");
    const database = new DatabaseSync(":memory:");
    try {
      database.exec("CREATE TABLE probe (value INTEGER NOT NULL)");
      database.prepare("INSERT INTO probe (value) VALUES (?)").run(41);
      const row = database.prepare("SELECT value FROM probe").get() as
        | { value?: unknown }
        | undefined;
      const versionRow = database.prepare("SELECT sqlite_version() AS version").get() as
        | { version?: unknown }
        | undefined;
      const value = typeof row?.value === "number" ? row.value : Number(row?.value);
      return {
        ok: true,
        version: String(versionRow?.version ?? ""),
        roundTrip: Number.isFinite(value) ? value : -1,
      };
    } finally {
      database.close();
    }
  } catch (error) {
    return { ok: false, error: describeError(error) };
  }
}

/**
 * `node-pty` 在 Windows 上**不做**任何「加载绑定」的公开入口：`exports.native` 是
 * `process.platform !== "win32" ? loadNativeModule("pty").module : null`（真源码见
 * `node_modules/node-pty/lib/index.js`），Win32 侧的原生模块只有 `spawn` 时经
 * `windowsPtyAgent` 懒加载。所以「没起进程就没法证绑定可用」——加载档改为断言
 * **原生 artifact 在磁盘上存在**（搜索顺序与 node-pty 自己的 `loadNativeModule` 一致：
 * `build/Release` → `build/Debug` → `prebuilds/<platform>-<arch>`，相对包根或包根的上级）。
 * 真起进程的强证据走 `probePtySpawn`（测试与 `MODE_HOST_NATIVE_DEP_PROBE=spawn` 档）。
 */
function findPtyNativeArtifacts(packageEntry: string): string[] {
  const moduleDir = dirname(packageEntry);
  const roots = [resolve(moduleDir, ".."), moduleDir];
  const directories = [
    join("build", "Release"),
    join("build", "Debug"),
    join("prebuilds", `${process.platform}-${process.arch}`),
  ];
  const found: string[] = [];
  for (const root of roots) {
    for (const directory of directories) {
      for (const name of ["conpty", "pty"]) {
        const candidate = join(root, directory, `${name}.node`);
        if (!existsSync(candidate)) continue;
        const label = join(directory, `${name}.node`);
        if (!found.includes(label)) found.push(label);
      }
    }
  }
  return found;
}

export function probePty(): PtyProbeResult {
  try {
    const entry = require_.resolve("node-pty");
    const pty = require_("node-pty") as {
      spawn?: unknown;
      fork?: unknown;
      createTerminal?: unknown;
    };
    for (const name of ["spawn", "fork", "createTerminal"] as const) {
      if (typeof pty[name] !== "function") {
        return { ok: false, error: `node-pty 加载成功但 ${name} 不是函数（API 面不符）` };
      }
    }
    const nativeArtifacts = findPtyNativeArtifacts(entry);
    if (nativeArtifacts.length === 0) {
      return {
        ok: false,
        error:
          "node-pty 的三个 API 都在，但找不到任何原生 .node artifact" +
          "（build/Release、build/Debug、prebuilds/<platform>-<arch> 全空）",
      };
    }
    return { ok: true, spawnVerified: false, nativeArtifacts };
  } catch (error) {
    return { ok: false, error: describeError(error) };
  }
}

/** 真起一个 pty 跑一条命令：这是「可用」的强证据，也是产品启动时不做的那一步。 */
export async function probePtySpawn(timeoutMs = 15_000): Promise<PtyProbeResult> {
  const loaded = probePty();
  if (!loaded.ok) return loaded;
  const pty = require_("node-pty") as {
    spawn: (
      file: string,
      args: string[],
      options: Record<string, unknown>,
    ) => {
      onData: (listener: (data: string) => void) => void;
      onExit: (listener: (event: { exitCode: number }) => void) => void;
    };
  };
  try {
    const shell = process.env.ComSpec?.trim() || process.env.COMSPEC?.trim() || "cmd.exe";
    const child = pty.spawn(shell, ["/c", "echo pty-ok"], {
      name: "xterm-color",
      cols: 80,
      rows: 24,
      cwd: process.cwd(),
      env: process.env,
    });
    return await new Promise<PtyProbeResult>((resolveProbe) => {
      let output = "";
      let settled = false;
      const timer = setTimeout(() => {
        if (settled) return;
        settled = true;
        resolveProbe({ ok: false, output, error: `pty 在 ${timeoutMs}ms 内没有退出` });
      }, timeoutMs);
      child.onData((data) => {
        output += data;
      });
      child.onExit(({ exitCode }) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        resolveProbe({ ok: true, spawned: true, spawnVerified: true, exitCode, output });
      });
    });
  } catch (error) {
    return { ok: false, error: describeError(error) };
  }
}

export function probeCua(env: NodeJS.ProcessEnv = process.env): CuaProbeResult {
  const injected = env[MODE_CUA_BUNDLED_HELPER_APP_PATH_ENV]?.trim();
  if (!injected) return { injected: false, ok: false };
  if (!existsSync(injected)) {
    return {
      injected: true,
      ok: false,
      path: injected,
      error: `${MODE_CUA_BUNDLED_HELPER_APP_PATH_ENV} 指向的路径不存在`,
    };
  }
  return { injected: true, ok: true, path: injected };
}

/**
 * 同步档探针（sidecar 启动时调用）：`node:sqlite` 真读写 + `node-pty` 加载检查 + CUA 路径检查。
 * 真起 pty 的强证据走 `probePtySpawn`（测试用），理由见模块文档。
 */
export function probeNativeDeps(env: NodeJS.ProcessEnv = process.env): NativeDepsReport {
  return {
    sqlite: probeSqlite(),
    pty: probePty(),
    cua: probeCua(env),
  };
}
