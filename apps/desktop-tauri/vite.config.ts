/**
 * 骨架前端的构建配置。
 *
 * `root` 是 `frontend/`（源码，提交进仓库），产物落在 `dist/`（被根 .gitignore 忽略）。
 * `tauri.conf.json` 的 `build.frontendDist` 指向产物、`beforeBuildCommand` 跑本配置的构建，
 * 所以全新检出后直接 `tauri build` 也能产出前端——不需要把产物提交进仓库。
 */
import { defineConfig } from "vite";

export default defineConfig({
  root: "frontend",
  // Tauri 的 asset 协议按相对路径取资源；默认的 "/" 会让打包态资源 404
  // （与 packages/desktop 的 renderer 同一口径，那边是 file:// 加载）。
  base: "./",
  build: {
    outDir: "../dist",
    emptyOutDir: true,
    target: "es2022",
    sourcemap: true,
  },
  // 壳只加载本地资源，不开 dev server 的网络暴露面。
  server: { host: "127.0.0.1", port: 5175, strictPort: true },
});
