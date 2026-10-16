/**
 * 骨架前端的构建配置（M1 的**命令面验收台**）。
 *
 * `root` 是 `frontend/`（源码，提交进仓库），产物落在 `dist/dev/`（被根 .gitignore 忽略）。
 *
 * **为什么是 `dist/dev/` 而不是 `dist/`**（M3 Task 3 · DR-T3-2）：Tauri 只有一个 `frontendDist`
 * （`../dist`），而 `dist/` 的根现在是**真实产品 UI**（`scripts/build-renderer-for-tauri.ts`
 * 把 renderer 的构建产物拷进去）。验收台降为**开发/诊断入口**：`dist/dev/index.html`，
 * 用 `MODE_TAURI_SHELL_ENTRY=dev/index.html` 进（见 `crates/sidecar-host` 的 `pick_shell_entry`）。
 * 发布路径用 `--no-bench` 装配，产物里没有 `dev/`——它多占体积，并把内部命令面暴露给用户。
 *
 * `tauri.conf.json` 的 `build.frontendDist` 指向装配后的根、`beforeBuildCommand` 跑装配脚本，
 * 所以全新检出后直接 `tauri build` 也能产出前端——不需要把产物提交进仓库。
 */
import { defineConfig } from "vite";

export default defineConfig({
  root: "frontend",
  // Tauri 的 asset 协议按相对路径取资源；默认的 "/" 会让打包态资源 404
  // （与 packages/desktop 的 renderer 同一口径，那边是 file:// 加载）。
  base: "./",
  build: {
    outDir: "../dist/dev",
    emptyOutDir: true,
    target: "es2022",
    sourcemap: true,
  },
  // 壳只加载本地资源，不开 dev server 的网络暴露面。
  server: { host: "127.0.0.1", port: 5175, strictPort: true },
});
