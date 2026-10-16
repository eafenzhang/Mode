/**
 * 把**真实产品 UI** 装配成 Tauri 的 `frontendDist`（M3 Task 3 · Step 3）。
 *
 * ## 为什么需要这一层（而不是直接改 `frontendDist`）
 *
 * `tauri.conf.json` 只有一个 `frontendDist`，而壳要加载两种页面：
 *
 * | 页面 | 落点 | 谁用 |
 * | --- | --- | --- |
 * | **产品页**（真 renderer 的构建产物） | `dist/index.html` + `dist/assets/**` | 默认入口（`WebviewUrl::App("index.html")`） |
 * | **命令面验收台**（M1 的回归工具） | `dist/dev/index.html` | 开发/诊断入口（DR-T3-2：保留但不进发布产物） |
 * | opt-in 集成测试页（测试资产） | `dist/harness/**` | `webviewRoundTrip.test.mts` 自己构建（本脚本不管） |
 *
 * 所以 `dist/` 是**装配出来的根**：本脚本先把真 renderer 的产物拷成根，
 * 再（默认）把验收台构到 `dist/dev/`。`--no-bench` 是发布路径用的档：
 * 验收台多占体积、并把内部命令面暴露在产物里，两样都不该出现在发布包中（DR-T3-2 的代价条款）。
 *
 * ## 两条有依据的取舍
 *
 * 1. **排除 `*.map`**：renderer 的构建默认产出 sourcemap（Electron 线靠它做符号化）。
 *    Tauri 的 `frontendDist` 会被**内嵌进可执行文件**，地图在那儿既没有消费者、
 *    又把整棵源码摊给任何解开产物的人。排除的是调试产物，不是运行期资产。
 * 2. **不 minify 二次处理**：renderer 产物已经是生产构建（`vite build --mode production`），
 *    再动它只会让「Tauri 看到的字节」与「Electron 看到的字节」分叉。
 *
 * ## 体积必须有出处（Ruling 26 的 (a) 核心预算）
 *
 * 脚本跑完打印**实测**字节数（产品 UI 资产 + 已存在的 Host bundle），那是「核心」那份预算的
 * 现基准。数字进 `docs/specs/tauri-renderer-host.md` 的证据登记；**不重新谈判门禁**。
 *
 * ```bash
 * node --import tsx apps/desktop-tauri/scripts/build-renderer-for-tauri.ts              # 产品页 + 验收台
 * node --import tsx apps/desktop-tauri/scripts/build-renderer-for-tauri.ts --no-bench   # 发布路径（无验收台）
 * node --import tsx apps/desktop-tauri/scripts/build-renderer-for-tauri.ts --skip-renderer-build
 * ```
 */
import { spawnSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readdirSync, rmSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const appDir = resolve(here, "..");
const repoRoot = resolve(appDir, "../..");
const rendererOut = resolve(repoRoot, "packages/desktop/out/renderer");
const distDir = resolve(appDir, "dist");
const benchOut = join(distDir, "dev");

const args = new Set(process.argv.slice(2));
const withBench = !args.has("--no-bench");
const skipRendererBuild = args.has("--skip-renderer-build");

const log = (message: string): void => console.log(`[renderer-for-tauri] ${message}`);
const mib = (bytes: number): string => `${bytes} B (${(bytes / 1024 / 1024).toFixed(2)} MiB)`;

/**
 * 用仓库自带的 vite（路径与版本都确定，不依赖 pnpm 的脚本名）。
 *
 * `cwd` 必须是被构建配置所属的包目录：`packages/desktop/vite.config.ts` 里有**相对 cwd**
 * 的字段（`root: "src/renderer"`、插件的相对路径），从别的目录跑会解析成错的地方
 * （实测：从 apps/desktop-tauri 跑会直接构建失败）。
 */
function runVite(configPath: string, cwd: string): void {
  const result = spawnSync(
    process.execPath,
    [resolve(repoRoot, "node_modules/vite/bin/vite.js"), "build", "--config", configPath],
    { cwd, stdio: "inherit" },
  );
  if (result.status !== 0) {
    throw new Error(`vite build 失败：${configPath}`);
  }
}

/** 递归求一个目录的大小与文件数（跳过 sourcemap，因为那正是我们不拷贝的那部分）。 */
function measure(dir: string): { bytes: number; files: number } {
  let bytes = 0;
  let files = 0;
  const walk = (current: string): void => {
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const full = join(current, entry.name);
      if (entry.isDirectory()) {
        walk(full);
      } else if (entry.isFile()) {
        bytes += statSync(full).size;
        files += 1;
      }
    }
  };
  if (existsSync(dir)) walk(dir);
  return { bytes, files };
}

function buildRenderer(): void {
  log("构建产品 renderer（packages/desktop 的 vite，mode=production）");
  runVite(resolve(repoRoot, "packages/desktop/vite.config.ts"), resolve(repoRoot, "packages/desktop"));
  if (!existsSync(join(rendererOut, "index.html"))) {
    throw new Error(`renderer 产物里没有 index.html：${rendererOut}`);
  }
}

/** 把 renderer 产物拷成 `dist/` 的根，排除 sourcemap（见文件头取舍 1）。 */
function assembleProductPage(): void {
  log(`装配 dist 根：${rendererOut} → ${distDir}（排除 *.map）`);
  mkdirSync(distDir, { recursive: true });
  cpSync(rendererOut, distDir, {
    recursive: true,
    filter: (source) => !source.endsWith(".map"),
  });
}

function buildBench(): void {
  log("构建命令面验收台（开发/诊断入口）→ dist/dev/");
  runVite(resolve(appDir, "vite.config.ts"), appDir);
  if (!existsSync(join(benchOut, "index.html"))) {
    throw new Error(`验收台产物里没有 index.html：${benchOut}`);
  }
}

function reportSize(): void {
  const product = measure(distDir);
  // 验收台在 dist/dev/ 下，量「产品 UI 资产」时要剔掉它（它按 DR-T3-2 不进发布产物）。
  const bench = measure(benchOut);
  const uiAssets = product.bytes - bench.bytes;
  log(`产品 UI 资产（含辅助页，不含 *.map）： ${mib(uiAssets)}`);
  log(`装配后的前端根 dist 合计：              ${mib(product.bytes)} 共 ${product.files} 个文件`);
  if (withBench) {
    log(`  其中验收台 dev/：                     ${mib(bench.bytes)}（发布路径用 --no-bench 排除）`);
  }
  const hostBundle = resolve(appDir, "dist-host-sidecar/host-sidecar/index.mjs");
  if (existsSync(hostBundle)) {
    const size = statSync(hostBundle).size;
    log(`Host bundle（Task 2 的产物，另行构建）：  ${mib(size)}`);
    log(`「核心」合计（UI 资产 + Host bundle）：   ${mib(uiAssets + size)}（不含壳 exe，壳由 Task 5 实测）`);
  } else {
    log("没有 dist-host-sidecar 的产物：先跑 build-host-sidecar.ts 才会有 Host bundle 那一项");
  }
}

function main(): void {
  if (!skipRendererBuild) buildRenderer();
  else log("跳过 renderer 构建（--skip-renderer-build）：复用 out/renderer 现有产物");
  // 先清 dist（`emptyOutDir` 只清它自己那一层，验收台在子目录里不会被清）。
  rmSync(distDir, { recursive: true, force: true });
  assembleProductPage();
  if (withBench) buildBench();
  else log("不构建验收台（--no-bench）：发布产物里没有 dev/ 入口");
  reportSize();
}

main();
