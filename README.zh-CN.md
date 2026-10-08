# Mode

<div align="center">
  <img src="packages/desktop/build/icons/512x512.png" alt="Mode" width="96" height="96" />
  <p><strong>桌面端、浏览器与终端三端的 AI 编程工作区——源自 Mode，独立维护。</strong></p>
</div>

<p align="center">
  简体中文 | <a href="README.md">English</a>
</p>

<p align="center">
  <a href="https://github.com/eafenzhang/Mode/releases"><img src="https://img.shields.io/github/v/release/eafenzhang/Mode?label=release" alt="Release" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT" /></a>
</p>

Mode 保留产品本身——一个陪你规划、改代码、跑命令、自我验证的编程智能体——并从公开源码重新构建：监控与遥测全部移除，官方平台服务整体下线。它是 [zai-org/Mode](https://github.com/zai-org/Mode) 的 Mode 审计分支的延续，来龙去脉记在 [NOTICE.md](NOTICE.md) 与提交历史里。

## 这个分支在做什么

- **没有监控，没有遥测**：客户端监控 SDK、用量与网络上报、崩溃采集、资源采样、UI 埋点全部删除，并加了防回归检查守住这些出口。
- **官方平台下线**：账号登录、套餐与额度、官方 MCP 凭证、反馈上传、官方插件市场都不再连接任何服务器，也没有开关能把它们打开。
- **插件目录随包离线**：插件商店的「公开」分段内置官方那份插件清单（含原版图标与中文描述），浏览不请求 CDN；「个人」分段放你自己登记的目录，例如 Claude Code 官方插件目录。
- **为日常使用补齐的东西**：IM 机器人、局域网远程连接、静默更新，见下。

## 功能一览

- **一个 Agent，三种界面**：Electron 桌面端、浏览器工作区与终端 TUI 共用同一个 Agent 运行时和同一批会话；远程工作区支持 SSH、WSL、Docker，也支持**局域网里的另一台 Mode**。
- **会规划、会改、会跑、会验证**：文件改动用 diff 呈现，终端命令带上下文，改完自己跑测试复核；需要网页任务时内置浏览器插件驱动真实浏览器。
- **动手之前先征求许可**：每一次编辑、命令和工具调用都可以要求审批——仅本次允许、本项目内一直允许，或完全放行。
- **IM 机器人**：在 微信 / 企业微信 / 飞书（中国）/ 钉钉 里直接驱动工作区。私聊按人、群聊按群各自绑定独立会话，回复流式发回聊天窗口，群聊可以保持安静（例如企业微信只在被 @ 时响应），可选心跳定时推送进展摘要。
- **局域网远程连接**：同一网络里跑着 Mode 的机器通过 UDP 广播被发现，用一次性 6 位配对码授权后即可当作远程主机使用——选一个对端工作区，像本地一样对话。
- **多智能体协作**：子代理、动态工作流、技能与定时自动化。
- **插件、技能与 MCP**：内置插件 + 带离线官方目录的插件市场；MCP 服务按用户或工作区配置。
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

前置：Git、Node.js **24.14.0**、pnpm **10.33.2**，版本以 [mise.toml](mise.toml) 为准。以下命令都在仓库根目录执行。

```bash
pnpm bootstrap                 # 安装依赖并准备本地桌面运行时资源
pnpm dev:desktop               # Electron 桌面端（默认生产配置；测试环境用 dev:desktop:test）
pnpm dev:web                   # 浏览器工作区
pnpm --filter @mode/cli dev   # Agent CLI
```

设置 `MODE_DATA_BASE_DIR` 可以让开发实例使用独立的数据目录，不动你正在用的那份。

提交前的常用校验：

```bash
pnpm typecheck                                              # TypeScript 工程引用
pnpm lint                                                   # oxlint
pnpm --filter @mode/services test                          # 服务与契约测试
node --test packages/desktop/tests/*.test.mjs               # 桌面端 node 测试
pnpm architecture:check -- --changed                        # 依赖方向策略检查
```

本地打一个桌面安装包（产物在 `packages/desktop/dist`）：

```bash
pnpm bundle:desktop -- --os win --arch x64
```

## 发版自动化

推送 `main` 就会自动构建并发布新版本：`.github/workflows/release.yml` 取最近一个正式版 tag 把 patch 号 +1（例如 `v0.0.1` → `v0.0.2`），先跑 `verify` 闸门（类型检查、Lint、服务与桌面端测试），再构建 Windows x64 / macOS Apple Silicon / Linux x64 三个桌面客户端，全部上传进一个 **draft** Release，等所有产物齐了才公开——构建失败就停在 draft，下载页永远不会解析到一个半成品版本。提交信息里带 `[skip release]` 可以只推代码不发版；需要指定版本号或发预发布时，手工触发这个 workflow。

## 仓库结构

| 路径                                                 | 内容                                                        |
| ---------------------------------------------------- | ----------------------------------------------------------- |
| `packages/desktop`                                   | Electron 主进程、host 与渲染端；局域网访问服务与发现        |
| `packages/ui`                                        | 共享 React 组件、hooks 与 Zustand store（设置页、插件商店） |
| `packages/services`                                  | 业务服务（Agent 会话、机器人、插件、远程连接）              |
| `packages/server`、`packages/web`                    | 浏览器工作区的服务端与客户端                                |
| `packages/client`、`packages/rpc`、`packages/shared` | Agent 客户端 SDK、RPC 框架、共享契约                        |
| `apps/mode-cli`                                     | Agent CLI 与运行时（桌面端也内嵌这套）                      |
| `harness/lan`                                        | 两台机器上试局域网远程连接的操作步骤                        |

## 许可与来源

本仓库第一方代码为 MIT（[LICENSE](LICENSE)），它 fork 的上游 Mode 代码为 Apache-2.0（[LICENSE-APACHE](LICENSE-APACHE)）。第三方组件及许可见 [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)；应用可能在你机器上做什么——文件读写、命令执行、钩子、浏览器自动化、后台任务——见 [NOTICE.zh-CN.md](NOTICE.zh-CN.md)（英文版 [NOTICE.md](NOTICE.md)）。

促成这个分支的遥测移除工作，连同它的验证边界，记录在[桌面端](packages/desktop/specs/telemetry-removal-report.md)、[CLI](apps/mode-cli/specs/telemetry-removal-report.md) 与 [UI](packages/ui/specs/telemetry-removal-report.md) 三份报告里。

## 反馈

问题、疑问与功能建议请[开 Issue](https://github.com/eafenzhang/Mode/issues)；先翻一下已有 Issue，并附上「设置 → 关于」里显示的版本号。
