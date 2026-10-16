/**
 * M3 Task 0 端口传输 harness 的页面构建配置（**测试资产**，不是产品前端）。
 *
 * 这是「端口传输 spike」的可复现验证面的一部分：spike 结论
 * （`docs/plans/2026-10-16-m3-port-transport-spike.md`）里的 inject → transfer → `Initialize`
 * 往返必须能被重跑，所以 harness 留在仓库里（测试拥有），而不是当成一次性脚本删掉。
 *
 * - `root` = `frontend/`，产物落 `apps/desktop-tauri/dist/harness/port-transport/`，
 *   由 Rust harness（`src-tauri/src/bin/port_transport_harness.rs`）以本地内容方式加载
 *   （远端 http 来源拿不到 app 命令，见 spike 文档 §4.2）。
 * - 三条 alias 都是为了在浏览器里加载仓库内的 **真实源码**：
 *   `@mode/rpc` → 包入口（真 `MessagePortProtocol`/`ChannelClient`）；
 *   `@mode/shared` → shim（barrel 会拉进 node 侧模块，见 shim 说明）；
 *   `@renderer-admission` → **未改动**的渲染层准入类源码。
 * - `/spike-log` 端点只用于诊断：WebView2 的 console 本机拿不到（M1 §4.3 已记录），
 *   页面/注入桥把日志 fetch 到这个 dev server 的 stdout——**独立于 Tauri invoke**，
 *   能把「页面跑没跑」与「invoke 通不通」分开看。
 */
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { defineConfig, type Plugin } from "vite";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../../../../..");

const spikeLog = (): Plugin => ({
  name: "spike-log",
  configureServer(server) {
    server.middlewares.use((req, res, next) => {
      const url = req.url ?? "";
      if (url.startsWith("/spike-log")) {
        const query = url.includes("?") ? url.slice(url.indexOf("?") + 1) : "";
        console.log(`SPIKE_PAGE_LOG ${decodeURIComponent(query)}`);
        res.statusCode = 204;
        res.setHeader("Access-Control-Allow-Origin", "*");
        res.setHeader("Access-Control-Allow-Methods", "GET, POST, OPTIONS");
        res.end();
        return;
      }
      next();
    });
  },
});

export default defineConfig({
  root: resolve(here, "frontend"),
  base: "./",
  plugins: [spikeLog()],
  resolve: {
    alias: {
      "@mode/rpc": resolve(repoRoot, "packages/rpc/src/index.ts"),
      "@mode/shared": resolve(here, "frontend/shared-shim.ts"),
      "@renderer-admission": resolve(
        repoRoot,
        "packages/desktop/src/renderer/src/databaseStartupAdmission.ts",
      ),
    },
  },
  build: {
    outDir: resolve(repoRoot, "apps/desktop-tauri/dist/harness/port-transport"),
    emptyOutDir: true,
    target: "es2022",
    sourcemap: true,
    // 刻意不压缩：harness 是测试资产，产物要能**当证据读**——压缩后 `MessagePortProtocol` /
    // `takeReadyPort` / `databaseStartupPortPayloadSchema` 这些真名字会被 mangle 掉，
    // 「页面引到的确实是那几份真源码」就只剩口头断言。
    minify: false,
  },
  server: { host: "127.0.0.1", port: 5176, strictPort: true },
});
