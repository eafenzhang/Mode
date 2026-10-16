/**
 * M3 Task 1 opt-in 集成测试页的构建配置（**测试资产**）。
 *
 * 与 Task 0 的 harness（`test/harness/port-transport/vite.config.ts`）同一套路，两点不同：
 *
 * - 页面加载的是**产品壳**（`mode_tauri::build_host_window` 注入的 `crates/sidecar-host/src/bridge.ts`）
 *   与**产品 Host**（`packages/desktop/src/host-standalone/index.ts`）——这里没有替身；
 * - 页面把结果 fetch 到一个由测试自己起的 HTTP sink（`127.0.0.1:5177`），因为 WebView2 的
 *   console 本机拿不到（M1 §4.3 记录），而这条通道**独立于 Tauri invoke**：
 *   invoke 全被拒时页面照样能报告它在做什么。
 *
 * alias 指向仓库里的**真实源码**：`@mode/rpc` 包入口、未改动的渲染层
 * `DatabaseStartupAdmission`、以及 Task 0 那份 shared-shim（barrel 会拉进 node 侧模块，
 * 浏览器里加载不起来，所以复用同一份重定向）。
 */
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { defineConfig } from "vite";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../../../../..");

export default defineConfig({
  root: resolve(here, "frontend"),
  base: "./",
  resolve: {
    alias: {
      "@mode/rpc": resolve(repoRoot, "packages/rpc/src/index.ts"),
      "@mode/shared": resolve(
        repoRoot,
        "apps/desktop-tauri/test/harness/port-transport/frontend/shared-shim.ts",
      ),
      "@renderer-admission": resolve(
        repoRoot,
        "packages/desktop/src/renderer/src/databaseStartupAdmission.ts",
      ),
    },
  },
  build: {
    outDir: resolve(repoRoot, "apps/desktop-tauri/dist/harness/host-sidecar"),
    emptyOutDir: true,
    target: "es2022",
    sourcemap: true,
    // 与 Task 0 的 harness 同一理由：产物要能当证据读，别 mangle 掉真名字。
    minify: false,
  },
});
