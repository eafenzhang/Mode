# spec: Mode 官网（mymode.cloud）

## 目标

为 Mode 提供官方网站：介绍产品、引导下载与查看源码。域名 `mymode.cloud`（apex 与 `www`）。

## 内容与结构

单页静态站，无构建步骤，无外部请求（自包含字体栈与图标，国内访问不依赖 CDN）。

1. 导航：logo、功能 / 三端 / 下载 / GitHub、语言切换（中文默认 / English）、主题切换（跟随系统，可手动覆盖）。
2. Hero：一句话定位 + 下载按钮 + 「签名」元素——循环播放的终端会话动画，演示 Mode「计划 → 改码 → 运行 → 验证 → 审批」闭环。
3. 功能网格（8 项）：验证闭环 / 审批授权 / 三端一体 / IM 机器人 / 局域网远程 / 远程被控端 / 插件技能 MCP / 自带模型 / 静默更新。
4. 三端展示：桌面端、浏览器、终端 TUI 各一张卡片。
5. 下载区：Windows / macOS / Linux 产物名与首次启动提示（未签名 + sha256 校验 + 自动更新），链接指向 GitHub Releases。
6. 页脚：GitHub / Releases / Issues / MIT。

产品事实以根目录 README 为准；不得虚构未实现的功能。

## 技术约束

- 位置 `website/`，不进入 pnpm workspace，不引入任何 npm 依赖。
- 交互仅原生 JS：i18n 字典切换、主题切换、终端打字动画（`prefers-reduced-motion` 时静态呈现）、版本号经 GitHub API 拉取（失败静默隐藏）。
- 视觉对齐 `DESIGN.md`：冷静、克制、信息密度优先；品牌色 sky（暗色 `#38bdf8` / 亮色 `#0ea5e9`）、中性底色（暗 `#161616` / 亮 `#f8f8f8`），不使用大面积品牌填充。
- 响应式覆盖桌面到手机；键盘焦点可见。

## 部署架构（所有者与链路）

- 状态所有者：站点文件唯一来源是仓库 `website/public/`；NAS 目录 `/vol1/docker/mymode-site/` 是运行时副本；DNS 与隧道 ingress 的所有者是 Cloudflare（zone `b46948a0c3898dc215c46f2763f731c3`，账号下远程托管隧道）。
- 链路：`浏览器 → Cloudflare 边缘（proxied）→ cloudflared 隧道（fnOS NAS）→ nginx 容器 127.0.0.1:8095 → /vol1/docker/mymode-site/`。
- apex 与 `www` 通过 CNAME（apex 依赖 Cloudflare 扁平化）指向 `<tunnel-id>.cfargotunnel.com`；切换时删除 apex 原有指向失效源站的 A 记录。隧道 ingress 需在既有 4 条业务路由（relay / agentgo / hub-cmd / gashub）之前插入两条新规则，catch-all 保持最后。
- 更新站点 = `website/deploy/deploy.sh`（tar 流传输到 NAS）；DNS / ingress 变更 = `website/deploy/setup-cloudflare.py`（幂等，可重复执行）。
- Cloudflare API token 不入库，运行时从 fnOS `/vol1/@appconf/com.dustinky.tunnel/config.json` 读取。

## 验收场景

1. `https://mymode.cloud` 与 `https://www.mymode.cloud` 返回官网首页（HTTP 200，HTTPS 由 Cloudflare 边缘终结）。
2. 已有业务子域（relay / agentgo / hub-cmd / gashub）路由不受影响。
3. 中英切换、暗亮主题切换、移动端布局可用；`prefers-reduced-motion` 下无打字动画。
4. Releases 链接指向 `https://github.com/eafenzhang/Mode/releases`。
