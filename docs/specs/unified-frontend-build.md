# 桌面 / Web 前端统一构建（安装包只带一份前端）

状态：已批准（2026-10-21，用户选择“统一桌面/Web 前端构建”解决安装包体积）。
关联：[remote-control-device-model.md](./remote-control-device-model.md)（Web/手机远控的静态资源挂载）、
[tauri-renderer-host.md](./tauri-renderer-host.md)（Tauri 壳与 Host 资源目录）。

## 背景

v0.0.73 为 Web/手机远控把 `packages/web/dist` 作为 Tauri 资源打进安装包，导致桌面端同时携带
两份几乎相同的前端：桌面 renderer（内嵌进 exe）与 Web renderer（资源目录）。去掉 source map 后
v0.0.74 仍比 v0.0.71 大 12–18MB，差额正好是一份 Web 前端（约 58MB 原始 / 18MB gzip）。

根因不是压缩参数，而是**两套构建产物**：`packages/desktop` 与 `packages/web` 各自用 Vite
构建同一套 `@mode/ui`，桌面端安装包把两份都装了进去。

## 产品规则

1. **一次构建、两个入口**：桌面 renderer 的 Vite 构建同时产出
   - `index.html`（Tauri 桌面入口，保持不变）；
   - `web.html`（浏览器 / LAN / 手机远控入口）。
   两个入口共享 Rollup 拆出的公共 chunk 与静态资源，安装包中只存在一份 UI。
2. **独立 Web 构建保留**：fnOS / `mode --web` 仍使用 `packages/web` 的独立构建
   （子路径 base、OAuth 等形态不变）；桌面安装包不再引用 `packages/web/dist`。
3. **Tauri 仍加载内嵌 `index.html`**：不改变窗口 URL、IPC 注入与启动顺序。
   Web 静态资源由壳在启动时从**内嵌资源**解压到本机缓存目录，再把
   `MODE_WEB_STATIC_ROOT` 传给 Host；Host 的 LAN 服务用同一份文件提供 `web.html`。
   - 解压目录按应用版本分代（`web-assets/<version>`），有 `.ready` 标记时跳过；
   - 解压失败不阻断桌面启动，只让 LAN Web 保持 API-only；
   - 不把资源再放进 `bundle.resources`，否则安装包又变回两份。
4. **Web 入口的 base 兼容**：统一构建的 `base` 是 `./`（Tauri/相对资源需要）。
   Web 入口在 `BASE_URL` 为相对值时，按当前页面目录推导 API/WS base；LAN 根路径下
   得到空 base（`/api`、`/ws`），独立 Web 构建仍使用 Vite 的 `MODE_WEB_BASE`。
5. **静态服务入口可配**：LAN 静态服务默认仍服务 `index.html`（独立 Web 构建）；
   桌面 LAN 服务显式指定 `web.html` 作为目录入口与 SPA 回退，避免把桌面入口返回给浏览器。

## 状态所有者与时序

```text
Vite（desktop 配置）
  ├─ index.html + desktop entry ── 内嵌进 Tauri exe ── Tauri webview
  └─ web.html   + web entry     ── 内嵌进 Tauri exe
                                      │
Tauri setup                           │
  ├─ extract embedded assets ──▶ <app_data>/web-assets/<version>/
  ├─ spawn Host with MODE_WEB_STATIC_ROOT=<...>
  └─ create window (WebviewUrl::App("index.html"))
                                      │
LAN / Web 请求 ──▶ Host staticRoot ──▶ web.html + 共享 assets
```

唯一所有者：构建入口与 chunk 由 Vite 构建决定；解压目录由 Tauri 壳拥有；静态服务只读该目录。

## 验收场景

1. `node --import tsx apps/desktop-tauri/scripts/build-renderer-for-tauri.ts --no-bench`
   的产物同时包含 `index.html`、`web.html`、一份共享 `assets/`，且无 `*.map`。
2. Tauri 窗口仍加载 `index.html`；LAN 服务 `/` 返回 `web.html`，`/assets/*` 命中同一份文件。
3. 浏览器打开 LAN 地址，Web 入口能完成配对、建立 WS、渲染 UI；桌面端不受影响。
4. 安装包不再包含 `packages/web/dist`；体积回到接近 v0.0.71（允许因 web 入口代码增加少量）。
5. `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed` 通过；
   `cargo check -p mode-tauri` 与 `cargo test -p mode-tauri --test web_assets_extraction` 通过
   （后者钉住「解压写盘的是 `Assets::get()` 的解压字节，而不是 `iter()` 的压缩字节」）。
6. **待实机**：安装后首次启动能解压并打开 Web 远控；升级后旧版本目录可清理。

## 非目标

- 不合并 `packages/desktop` 与 `packages/web` 的源码结构；两个入口仍各自引导。
- 不改变 Tauri 窗口加载内嵌页面的方式，也不把桌面 UI 改为 HTTP 加载。
- 不改变独立 Web/fnOS 构建的 base、OAuth 与部署形态。
- 不把 Web 资源改为联网下载。
