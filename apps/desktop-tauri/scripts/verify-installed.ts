/**
 * **装完之后的验证**（M3 Task 5 · Step 5）：真实安装包 → 静默装进隔离目录 → **覆盖重装** → 跑起来 → 用真终端。
 *
 * 这一层要回答的四件事（每一件都有原始输出入仓，见 spec §13.6）：
 *
 * 1. **产物里到底有什么**：产品 UI 的入口资产名必须能在安装后的壳 exe 里逐字找到
 *    （`frontendDist` 被 `tauri::generate_context!` 内嵌进 exe），且**验收台的资产名找不到**
 *    ——「出的是验收台配置的包却当成交付物」正是本任务点名要避免的那个错；
 *    `host-sidecar/index.mjs`、`node/node.exe`、`config/provider/mode-builtin.json` 必须落在安装目录里；
 * 2. **产品壳真跑得起来**：隔离数据基目录下启动，stderr 里出现 Host 的 `ready` 行与壳的
 *    `platform_surface` 行，且**没有** `startup_failed`；
 * 3. **剪枝必须实跑，不是报个数**：用**安装目录里的** `node.exe` 起**安装目录里的** sidecar，
 *    真开一个 pty 跑一条命令（`terminal` 频道端到端）；
 * 4. **数据隔离**：写入发生在 `{base}/.mode`，用户**真实**的 `~/.mode` 条目集合未被改动。
 *
 * **本文件只放「起壳跑一次」这条链与编排**：安装目录/安装包的判定在
 * `installed-artifacts.ts`，随包 sidecar 与终端探针在 `installed-sidecar-probe.ts`
 * （拆开的理由是 `oxlint` 的 `max-lines` 400——那 400 行是硬约束，不是为了好看）。
 *
 * ```bash
 * node --import tsx apps/desktop-tauri/scripts/verify-installed.ts
 * node --import tsx apps/desktop-tauri/scripts/verify-installed.ts --installer <path> --keep
 * node --import tsx apps/desktop-tauri/scripts/verify-installed.ts --install-only    # 给冷启动让出「安装后首次启动」
 * ```
 */
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  checkEmbeddedFrontend,
  findMainExeOrNull,
  installSilently,
  inventory,
  reinstallAndReport,
  sha256File,
  type CheckFn,
} from "./installed-artifacts.ts";
import { probeInstalledTerminal } from "./installed-sidecar-probe.ts";
import {
  entryNames,
  listProcessesUnder,
  runProductShell,
  type ShellTrace,
} from "./shell-process-probe.ts";

const here = dirname(fileURLToPath(import.meta.url));
const appDir = resolve(here, "..");
const repoRoot = resolve(appDir, "../..");

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

const checks: { name: string; ok: boolean; detail: string }[] = [];
const check: CheckFn = (name, ok, detail) => {
  checks.push({ name, ok, detail });
  console.log(`  [${ok ? "PASS" : "FAIL"}] ${name} — ${detail}`);
};
const mib = (bytes: number): string => `${bytes} B (${(bytes / 1024 / 1024).toFixed(2)} MiB)`;

/** 跑起来的产品壳：两个标记、无失败行、退出后无孤儿。 */
async function checkShellRuns(
  installDir: string,
  mainExe: string,
  workDir: string,
  nonce: string,
  realRootBefore: string[] | null,
): Promise<ShellTrace> {
  const dataBase = join(workDir, `shell-base-${nonce}`);
  const home = join(workDir, `shell-home-${nonce}`);
  mkdirSync(dataBase, { recursive: true });
  mkdirSync(home, { recursive: true });
  console.log(`[verify] 启动安装后的产品壳（隔离基目录 ${dataBase}）`);
  const trace = await runProductShell({
    exe: mainExe,
    dataBaseDir: dataBase,
    home,
    marks: [
      { key: "ready", needle: "[mode-host-sidecar] ready startupId=" },
      { key: "surface", needle: "[mode-tauri] platform_surface implemented=" },
    ],
    timeoutMs: 90_000,
    settleMs: 3_000,
  });
  console.log("  --- 壳的 stderr/stdout 原文 ---");
  for (const line of trace.lines)
    console.log(`  ${String(line.at).padStart(6)}ms [${line.stream}] ${line.text}`);
  console.log("  --- 原文结束 ---");

  const text = trace.lines.map((line) => line.text).join("\n");
  check(
    "Host 报出 ready（经壳的 stderr 转发）",
    trace.marks.ready !== undefined,
    `T_ready = ${String(trace.marks.ready)} ms`,
  );
  check(
    "注入层上报平台面清单（页面 + 注入 + IPC 都通了）",
    trace.marks.surface !== undefined,
    `T_surface = ${String(trace.marks.surface)} ms；` +
      (trace.lines.find((line) => line.text.includes("platform_surface"))?.text ?? "(无)"),
  );
  check(
    "没有 startup_failed",
    !text.includes("startup_failed"),
    "stderr 里没有 Host 的结构化失败行",
  );
  check(
    "收尾方式",
    true,
    trace.closedBy === "wm_close" ? "WM_CLOSE（正常退出路径）" : "强杀整棵进程树（退化档）",
  );

  const dataRoot = join(dataBase, ".mode");
  const rootEntries = entryNames(dataRoot);
  check(
    "真服务写进了隔离数据根",
    rootEntries !== null && rootEntries.length > 0,
    `${dataRoot} → ${JSON.stringify(rootEntries)}`,
  );
  // 隔离的第二半：用户真实数据根没被碰（存在性 + 条目集合）。真实根在这台机器上可能不存在，
  // 那就断言它**仍然**不存在——「没被创建」也是没被碰。
  const realRoot = join(homedir(), ".mode");
  const after = entryNames(realRoot);
  check(
    "宿主的真实 ~/.mode 未被本轮的隔离跑改动",
    JSON.stringify(after) === JSON.stringify(realRootBefore),
    `跑之前 ${JSON.stringify(realRootBefore)} → 跑之后 ${JSON.stringify(after)}`,
  );

  // 壳退出后不该有安装目录下的 node.exe 残留（两种收尾方式都不该有）。
  await sleep(2_000);
  const orphans = listProcessesUnder("node.exe", installDir);
  check(
    "退出后没有安装目录里的 node.exe 残留",
    orphans.length === 0,
    orphans.length === 0 ? "0 个" : orphans.map((p) => `${p.pid} ${p.path}`).join(", "),
  );
  return trace;
}

const realRootBefore = entryNames(join(homedir(), ".mode"));

async function main(): Promise<void> {
  const argv = process.argv.slice(2);
  const valueOf = (flag: string): string | undefined => {
    const index = argv.indexOf(flag);
    return index >= 0 ? argv[index + 1] : undefined;
  };
  const keep = argv.includes("--keep");
  const installer = resolve(
    valueOf("--installer") ??
      join(appDir, "dist-host-sidecar", "installers", "Mode-0.0.1-win-x64-pruned.exe"),
  );
  if (!existsSync(installer))
    throw new Error(`安装包不存在：${installer}（先跑 measure:installer-budgets）`);
  const nonce = `${String(process.pid)}${String(Date.now() % 100000)}`;
  const workDir = resolve(valueOf("--work-dir") ?? join(tmpdir(), `mode-m3t5-verify-${nonce}`));
  const installDir = resolve(valueOf("--install-dir") ?? join(workDir, "install"));
  mkdirSync(workDir, { recursive: true });

  console.log(`[verify] 安装包 ${installer} ${mib(statSync(installer).size)}`);
  // 主 exe 的名字是 **Cargo 的 bin 名**（`ModeTauri.exe`），不是 `productName`——写死别的名字
  // 会得到「装完了却报找不到 exe」（见 `findMainExe` 的说明）。
  let mainExe = findMainExeOrNull(installDir);
  if (mainExe === null) {
    mainExe = await installSilently(installer, installDir);
    console.log(`[verify] 首次安装完成，主 exe = ${mainExe}`);
  } else {
    console.log(`[verify] 已装过，复用 ${installDir}（主 exe = ${mainExe}）`);
  }
  console.log(`[verify] installDir=${installDir}`);
  // `--install-only`：只装不动（给 `measure-cold-start.ts` 让出「安装后首次启动」那一刻）。
  if (argv.includes("--install-only")) {
    console.log("[verify] --install-only：装完即止");
    return;
  }

  // 覆盖重装：这一步是 Step 5 的点名要求（安装 → 重装 → 跑），不是重复劳动。
  const beforeReinstall = { sha256: sha256File(mainExe), bytes: statSync(mainExe).size };
  mainExe = await reinstallAndReport(installer, installDir, beforeReinstall, check);

  console.log("");
  console.log("[verify] === 2. 安装目录里到底有什么 ===");
  for (const row of inventory(installDir)) console.log(`  ${mib(row.bytes)}  ${row.path}`);
  const sidecar = join(installDir, "host-sidecar", "index.mjs");
  const bundledNode = join(installDir, "node", "node.exe");
  const builtinConfig = join(installDir, "config", "provider", "mode-builtin.json");
  check("安装目录里有 Host sidecar", existsSync(sidecar), sidecar);
  check("安装目录里有随包 Node", existsSync(bundledNode), bundledNode);
  check("安装目录里有 Built-in Provider Config", existsSync(builtinConfig), builtinConfig);
  // 远程桌面媒体面与 CUA 都经 Windows helper；纯 Node sidecar 的解析基准是
  // `process.resourcesPath/tools/cua-helper`（由 Host 从 MODE_HOST_RESOURCES_DIR 桥接）。
  // 缺这三件时会话能建立，但 `rd_media_start` 固定失败（主控显示「对端不支持」）。
  for (const name of ["entry.cjs", "cua_ax.node", "runtime-manifest.json"]) {
    const helperFile = join(installDir, "tools", "cua-helper", name);
    check(`安装目录里有 CUA helper（${name}）`, existsSync(helperFile), helperFile);
  }
  checkEmbeddedFrontend(mainExe, installer, appDir, check);
  // 发布产物里不该有验收台的 dev/ 入口（§17 的 DR-T3-2 代价条款）。
  check(
    "安装目录里没有验收台（dev/）",
    !existsSync(join(installDir, "dev")),
    `dist/dev 未进资源目录：${installDir}`,
  );

  console.log("");
  console.log("[verify] === 3. 产品壳跑得起来（隔离数据基目录）===");
  await checkShellRuns(installDir, mainExe, workDir, nonce, realRootBefore);

  console.log("");
  console.log("[verify] === 4. 用安装目录里的 node + sidecar 真开一个终端 ===");
  const terminal = await probeInstalledTerminal({
    installDir,
    workDir,
    nonce,
    ptyCwd: repoRoot,
    check,
  });
  writeFileSync(join(workDir, "terminal-output.txt"), terminal.output, "utf8");

  const failed = checks.filter((entry) => !entry.ok);
  console.log("");
  console.log(
    `[verify] === 结论：${String(checks.length - failed.length)}/${String(checks.length)} 条通过 ===`,
  );
  if (failed.length > 0) {
    for (const entry of failed) console.log(`  FAIL ${entry.name} — ${entry.detail}`);
  }

  // 收尾：默认静默卸载（NSIS 会同时清掉 HKCU 的卸载登记项），`--keep` 时留着供人工查看。
  if (!keep) {
    const uninstaller = join(installDir, "uninstall.exe");
    if (existsSync(uninstaller)) {
      // NSIS 卸载器会自己复制一份到临时目录再跑，所以这里不等它结束，只等文件消失。
      spawnSync(uninstaller, ["/S"], { stdio: "ignore" });
      await new Promise((resolve) => setTimeout(resolve, 5_000));
    }
    try {
      rmSync(workDir, { recursive: true, force: true, maxRetries: 5, retryDelay: 300 });
      console.log(`[verify] 已卸载并清理 ${workDir}`);
    } catch (error) {
      console.log(`[verify] 清理失败（不影响结论）：${String(error)}`);
    }
  } else {
    console.log(`[verify] --keep：安装与数据留在这里 ${workDir}`);
  }
  process.exitCode = failed.length === 0 ? 0 : 1;
}

void main().catch((error: unknown) => {
  console.error(
    `[verify] 失败：${error instanceof Error ? (error.stack ?? error.message) : String(error)}`,
  );
  process.exitCode = 1;
});
