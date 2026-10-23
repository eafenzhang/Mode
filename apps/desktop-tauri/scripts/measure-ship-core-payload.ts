/**
 * 发运集**核心载荷**的观测器（路线图 M4a：「发布门禁补『发运集核心载荷 ≤ 25 MB』的观测项」）。
 *
 * ## 这份脚本回答的是哪一个数
 *
 * M4 目标表「轻」行的核心载荷：**host-sidecar + agent + tools + config 的未压缩字节和**。
 * 与它同表的另外两个数是不同测量面，不可混用：
 * - 安装包字节（NSIS，`tauri-bundle-report.ts` 记的那个）——压缩后的单一安装件；
 * - `node/` 89 MB——单独预算 (b)，M4 目标「Node 运行时 89 MB → 0」单列，不进核心数。
 *
 * 2026-10-23 基线：≈63 MB（host-sidecar 35 + agent 20 + tools 8.6 + config 0.2）。
 *
 * ## 只记录，不阻断
 *
 * DR-M5-2b：体积不作为发布阻断。本脚本**永远 exit 0**（除输入缺失），
 * 超 25 MB 只在输出与 job summary 里标注 `over-target`——数字先落库，
 * 阈值转正是路线图层面的决策，不在脚本里私自加。
 *
 * ```bash
 * node --import tsx apps/desktop-tauri/scripts/measure-ship-core-payload.ts
 * # 输出：dist/tauri/ship-core-payload.txt + stdout（CI 侧再进 GITHUB_STEP_SUMMARY）
 * ```
 */
import { appendFileSync, existsSync, mkdirSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

/** 核心载荷的目录构成（相对 ship 根）；`node/` 刻意不在内——它是预算 (b)。 */
const CORE_DIRECTORIES = ["host-sidecar", "agent", "tools", "config"] as const;
/** 路线图 M4 目标值：核心载荷 ≤ 25 MB（1 MB = 1024² 字节，与其余体积口径一致）。 */
const TARGET_MB = 25;
/**
 * 路径一律相对**本脚本**解析，不用 `process.cwd()`：
 * ship 集的真实落点是 `apps/desktop-tauri/dist-host-sidecar`（`build-host-sidecar.ts`
 * 的 OUT_DIR）。cwd 从仓库根跑会指到不存在的 `<repo>/dist-host-sidecar`——
 * CI 的观测步正是从仓库根调用，cwd 口径在那边必错（2026-10-23 夹具实测踩到）。
 */
const APP_DIR = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const SHIP_ROOT = resolve(APP_DIR, "dist-host-sidecar/ship");
/** 报告落盘跟安装包体积记录同一目录（仓库根 `dist/tauri/`，与 CI 产物收集一致）。 */
const OUTPUT_FILE = resolve(APP_DIR, "../../dist/tauri/ship-core-payload.txt");

/** 递归求目录的文件字节数和；只认常规文件（symlink 不跟随，避免把宿主机器的链接算进来）。 */
function directoryBytes(dir: string): { bytes: number; files: number } {
  let bytes = 0;
  let files = 0;
  const stack = [dir];
  while (stack.length > 0) {
    const current = stack.pop() as string;
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const full = join(current, entry.name);
      if (entry.isDirectory()) {
        stack.push(full);
      } else if (entry.isFile()) {
        bytes += statSync(full).size;
        files += 1;
      }
    }
  }
  return { bytes, files };
}

function mib(bytes: number): string {
  return (bytes / (1024 * 1024)).toFixed(2);
}

function main(): void {
  if (!existsSync(SHIP_ROOT)) {
    // 输入缺失要响亮失败：CI 里 ship 集没 stage 出来本身就是构建缺陷，
    // 静默产出 0 MB 会把「达标」写成假绿。
    console.error(
      `[ship-core-payload] 发运集不存在：${SHIP_ROOT}（先跑 build:package-inputs --stage）`,
    );
    process.exit(1);
  }

  const rows: { name: string; bytes: number; files: number }[] = [];
  for (const name of CORE_DIRECTORIES) {
    const dir = join(SHIP_ROOT, name);
    if (!existsSync(dir)) {
      // 缺目录按 0 计但显式标出：tools 在非 Windows 目标可能缺席（cua-helper 只随 Windows 腿）。
      rows.push({ name, bytes: 0, files: 0 });
      console.warn(`[ship-core-payload] 警告：${name}/ 不存在，按 0 计`);
      continue;
    }
    const { bytes, files } = directoryBytes(dir);
    rows.push({ name, bytes, files });
  }

  const totalBytes = rows.reduce((sum, row) => sum + row.bytes, 0);
  const totalMb = totalBytes / (1024 * 1024);
  const overTarget = totalMb > TARGET_MB;

  const lines = [
    `ship core payload (uncompressed directory bytes; face=dist-host-sidecar/ship/{${CORE_DIRECTORIES.join(",")}}; node/ excluded — budget (b))`,
    ...rows.map((row) => `  ${row.name.padEnd(14)} ${mib(row.bytes).padStart(8)} MiB  (${String(row.files)} files)`),
    `  ${"total".padEnd(14)} ${mib(totalBytes).padStart(8)} MiB  (target <= ${String(TARGET_MB)} MiB${overTarget ? " — OVER TARGET (recorded only, DR-M5-2b)" : ""})`,
  ];
  const report = `${lines.join("\n")}\n`;

  mkdirSync(dirname(OUTPUT_FILE), { recursive: true });
  writeFileSync(OUTPUT_FILE, report);
  console.log(report);
  if (process.env.GITHUB_STEP_SUMMARY) {
    appendFileSync(
      process.env.GITHUB_STEP_SUMMARY,
      `### Ship core payload (roadmap M4a; recorded only)\n\`\`\`\n${report}\`\`\`\n`,
    );
  }
  console.log(`[ship-core-payload] 已写入 ${OUTPUT_FILE}`);
}

main();
