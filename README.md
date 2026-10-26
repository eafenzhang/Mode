# Mode

<div align="center">
  <img src="apps/desktop-tauri/src-tauri/icons/icon.png" alt="Mode" width="96" height="96" />
  <p><strong>桌面端、浏览器与终端三端的 AI 编程工作区，核心以 Rust 重写。</strong></p>
</div>

<p align="center">
  简体中文 | <a href="README.en.md">English</a> | <a href="https://mymode.cloud/">官网</a>
</p>

<p align="center">
  <a href="https://github.com/eafenzhang/Mode/releases"><img src="https://img.shields.io/github/v/release/eafenzhang/Mode?label=release" alt="Release" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT" /></a>
</p>

Mode 是一个陪你规划、改代码、跑命令、自我验证的编程智能体。桌面端以 Rust + Tauri v2 重建：窗口与平台能力、协议真源、更新 / 托盘 / 深链、Host sidecar 监督都落在 Rust workspace（`crates/`）里，Agent 会话运行时与工具执行的重写对应 `crates/agent-core` 与 `crates/agent-tools`；界面复用共享 React 组件，插件、IM 机器人等生态能力由随包的 Host sidecar 承载，按路线图逐步并入 Rust。

## 功能一览

- **一个 Agent，三种界面**：Tauri 桌面端、浏览器工作区与终端 TUI 共用同一个 Agent 运行时和同一批会话；远程工作区支持 SSH、WSL、Docker，也支持**局域网里的另一台 Mode**。
- **会规划、会改、会跑、会验证**：文件改动用 diff 呈现，终端命令带上下文，改完自己跑测试复核；需要网页任务时内置浏览器插件驱动真实浏览器。
- **动手之前先征求许可**：每一次编辑、命令和工具调用都可以要求审批——仅本次允许、本项目内一直允许，或完全放行。
- **IM 机器人**：在 微信 / 企业微信 / 飞书（中国）/ 钉钉 里直接驱动工作区。私聊按人、群聊按群各自绑定独立会话，回复流式发回聊天窗口，群聊可以保持安静（例如企业微信只在被 @ 时响应），可选心跳定时推送进展摘要。
- **局域网远程连接**：同一网络里跑着 Mode 的机器通过 UDP 广播被发现，用一次性 6 位配对码授权后即可当作远程主机使用——选一个对端工作区，像本地一样对话。
- **远程被控端**：Windows（XP 至 11）机器可运行单文件 `mode-agent`，经标准 MCP Streamable HTTP 接受指令——命令与后台作业、文件、进程、截屏与键鼠控制，详见 [apps/remote-agent](apps/remote-agent/README.md)。
- **多智能体协作**：子代理、动态工作流、技能与定时自动化。
- **插件、技能与 MCP**：内置插件 + 插件市场；「公开」分段随包内置离线目录（含图标与中文描述），浏览不请求 CDN，「个人」分段放你自己登记的目录，例如 Claude Code 插件目录。MCP 服务按用户或工作区配置。
- **自带模型**：内置 DeepSeek、OpenAI、Anthropic、Moonshot Kimi、MiniMax、Z.AI（GLM）、阿里、xAI、小米 MiMo、OpenRouter 预设，也支持完全自定义端点（Chat Completions / Responses / Anthropic Messages）。
- **静默更新**：应用自己检查本仓库的 GitHub Releases，遵循你设置的代理，按你的设置退出时安装。

## 下载与安装

安装包都在 [Releases](https://github.com/eafenzhang/Mode/releases) 页面。产物**没有代码签名**，各系统首次启动都会拦一次——这是预期行为，下载后可以对照同页面的 `sha256.txt` 自行校验（Windows `certutil -hashfile <文件> SHA256`，macOS `shasum -a 256 <文件>`，Linux `sha256sum <文件>`）。

| 平台                | 产物                                | 首次启动                                                                                  |
| ------------------- | ----------------------------------- | ----------------------------------------------------------------------------------------- |
| Windows x64         | `Mode-<版本>-win-x64.exe`           | SmartScreen 提示时点「更多信息」→「仍要运行」                                             |
| macOS Apple Silicon | `Mode-<版本>-mac-arm64.dmg`         | `sudo /usr/bin/xattr -rd com.apple.quarantine "/Applications/Mode.app" && open -a "Mode"` |
| Linux x86_64        | `Mode-<版本>-linux-x86_64.AppImage` | `chmod +x` 后直接运行                                                                     |

已安装的桌面端会从本仓库的 Releases 自动更新。

SSH / WSL / Docker 远端工作区会复用主机上已有的运行时；本仓库不发布预构建的远端运行资源。要给一台什么都没有的主机做首次部署，可以本地执行 `pnpm prepare:remote-assets` 自己准备，并用 `MODE_REMOTE_ASSET_CDN_BASE_URL` 指向你的存放位置。

## 从源码运行

前置：Git、Node.js **24.14.0**、pnpm **10.33.2**（以 [mise.toml](mise.toml) 为准）、Rust stable 工具链（[rust-toolchain.toml](rust-toolchain.toml)，含 clippy 与 rustfmt）。以下命令都在仓库根目录执行。

```bash
pnpm bootstrap                 # 安装依赖并构建 workspace 包
pnpm dev:desktop               # Tauri 桌面端（Rust 壳 + 共享 renderer）
pnpm dev:web                   # 浏览器工作区
pnpm --filter @mode/cli dev   # Agent CLI
```

设置 `MODE_DATA_BASE_DIR` 可以让开发实例使用独立的数据目录，不动你正在用的那份；`mise run dev` 已内置该隔离。

提交前的常用校验：

```bash
pnpm typecheck                            # TypeScript 工程引用
pnpm lint                                 # oxlint
cargo test --workspace                    # Rust workspace 测试
cargo clippy --all-targets -- -D warnings # Rust 静态检查
pnpm architecture:check -- --changed      # 依赖方向策略检查
```

Windows 本机没有 VS Build Tools 时，Rust 命令可改走 `mise run rust -- test --workspace`（自动装配 gnu 工具链；CI 与常规环境直接用 `cargo` 即可）。

本地打一个桌面安装包（产物在 `dist/tauri`）：

```bash
pnpm --filter @mode/desktop-tauri run tauri:build
```

## 发版自动化

推送 `main` 就会自动构建并发布新版本：`.github/workflows/release.yml` 取最近一个正式版 tag 把 patch 号 +1（例如 `v0.0.1` → `v0.0.2`），先跑 `verify` 闸门（类型检查、Lint、服务与桌面端测试），再构建 Windows x64 / macOS Apple Silicon / Linux x64 三个桌面客户端，全部上传进一个 **draft** Release，等所有产物齐了才公开——构建失败就停在 draft，下载页永远不会解析到一个半成品版本。提交信息里带 `[skip release]` 可以只推代码不发版；需要指定版本号或发预发布时，手工触发这个 workflow。

## 仓库结构

| 路径                                                 | 内容                                                                              |
| ---------------------------------------------------- | --------------------------------------------------------------------------------- |
| `crates/`                                            | Rust workspace：协议真源（`protocol`）、会话运行时（`agent-core`、`agent-tools`）、平台命令面（`platform`）、Host sidecar 监督（`sidecar-host`）、插件宿主、更新器、托盘、深链等 |
| `apps/desktop-tauri`                                 | Tauri v2 桌面壳（Rust 命令面、打包）                                              |
| `apps/remote-agent`                                  | Windows 远控被控端 `mode-agent`（Go，XP 至 11）                                   |
| `apps/mode-cli`                                      | Agent CLI 与运行时                                                                |
| `packages/desktop`                                   | 产品 renderer 与 Host sidecar 源码；局域网访问服务与发现                          |
| `packages/ui`                                        | 共享 React 组件、hooks 与 Zustand store（设置页、插件商店）                       |
| `packages/services`                                  | 业务服务（Agent 会话、机器人、插件、远程连接）                                    |
| `packages/web`、`packages/server`                    | 浏览器工作区的客户端与服务端                                                      |
| `packages/client`、`packages/rpc`、`packages/shared` | Agent 客户端 SDK、RPC 框架、共享契约                                              |
| `docs/specs`                                         | 产品规则、协议与验收场景                                                          |

## 许可与说明

本仓库第一方代码以 MIT 发布（[LICENSE](LICENSE)）；源自 Apache-2.0 项目的代码沿用其许可（[LICENSE-APACHE](LICENSE-APACHE)）。第三方组件及许可见 [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)；应用可能在你机器上做什么——文件读写、命令执行、钩子、浏览器自动化、后台任务——以及对应边界，见 [NOTICE.zh-CN.md](NOTICE.zh-CN.md)（英文版 [NOTICE.md](NOTICE.md)）。

## 反馈

问题、疑问与功能建议请[开 Issue](https://github.com/eafenzhang/Mode/issues)；先翻一下已有 Issue，并附上「设置 → 关于」里显示的版本号。
