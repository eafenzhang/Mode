# 剩余改造长任务清单（2026-11-23 刷新）

> 上位：`docs/plans/2026-10-29-full-rust-rewrite.md` §10、
> `docs/plans/2026-11-16-upstream-capability-long-tasks.md`。
> 每个 LT 开工前先补该任务的 spec/细计划；本文件只做 program-level 排序与出口定义。
> 剩余生态面已收敛为单一长任务 **LT-15**（P1–P6），执行状态见
> `2026-11-21-lt15-ecosystem-closeout.md`。
> **2026-11-23 刷新依据**：`011e7e388..6e09a49b7`（72 提交）逐项复核源码，不采信旧状态文字；
> 本轮活动集中在 **LT-16**，LT-7 ~ LT-14 无代码提交（`crates/tui`、
> `apps/mode-cli/packages/{tui,eval}`、`scripts/im-sdk-runtime` 零改动）。

## 总览

| ID | 长任务 | 依赖 | 出口条件 | 状态 |
| --- | --- | --- | --- | --- |
| **LT-7** | IM SDK 在线升级发布侧 | 无 | Release 挂载 `manifest.json` / `.sig` / `bundle.tar.gz`；配置公钥后客户端可在线激活 | 🟡 工具链完成；签名 secret、客户端公钥、设置页、壳资源注入待做。2026-11-23 复核：客户端读 `MODE_IM_SDK_RUNTIME_MANIFEST_URL` / `MODE_IM_SDK_RUNTIME_PUBKEY` / `MODE_IM_SDK_RUNTIME_DISABLE`（`packages/services/src/bots/imSdkRuntime.ts`），`host-standalone/index.ts` 已挂 `maybeRefreshImSdkRuntimeInBackground()`（不阻塞 ready）；**壳侧未注入这两个 env**（Tauri 源码零引用），设置页无版本/检查更新入口 |
| **LT-8** | W5 TUI 改 ratatui | 无强依赖 | 33 个场景清单逐项过 + 真机交互验收 | 🟡 无新进展（11-21 后 `crates/tui` 零提交）：视图模型/渲染/入口 + sessions-index 归约 + Agent stdio 传输（订阅/live/Ctrl-N）已落，14 单测 + 1 e2e（spec `tui-ratatui.md`）；**发送面仍未接**——`main.rs` Enter 分支只做本地回显（注释「发送面（session/send）在后续切片接入」），33 场景未过 |
| **LT-9** | W5 Web/远程服务端改 axum | W2/W4 稳定 | 契约对拍 + 手机远控验收 | 🟡 HTTP + WS 已全收口：axum health/version/server-info + LAN 六端点 + **`/ws/host` 帧桥**（`crates/server/src/ws_host.rs`，一次性 capability 30s TTL + 每连接 `ChannelServer` + 复用 `host_channels::assembly`；`crates/server/src/main.rs` 同时挂 `router_with_lan` 与 `router_with_ws_host`）；**剩余**：手机远控真机验收、`/ws/v1/direct`（v3 ControlLink） |
| **LT-10** | W5 插件市场逻辑进壳 | `plugin-host` 资产层 | 目录/安装/卸载走壳，素材仍按资产层 | 🟡 源码已远超原状态：`crates/plugin-sync-service`（14 文件）覆盖记录层（5 测试）、卸载编排、安装编排（依赖闭包 BFS/跨市场阻断/循环检测）、config 写回（enable/remove/suppression，5 测试）、网络源（github/git/url/git-subdir 走 `git clone` 子进程：`MODE_GIT_BINARY`、90s 超时、网络型 3 次重试）、**zip 源**（sha256 + 四重限额 + 根定位，7 测试）；`npm`/`pip` 显式不支持（与 TS 同口径）。**剩余**：真机走查与 TS 逐字段对拍补齐 |
| **LT-11** | W6 收口 | LT-7 ~ LT-10 | `stdioFrames`/`stdoutGuard` 退场、删 TS 宿主编排与 `host-sidecar` 发运集、插件宿主换嵌入引擎、四仪表终值复测 | 🟡 `node scripts/w6-readiness.mjs`（2026-11-23 实跑，扫描 5871 文件）仍 **NOT READY**：stdioFrames 6 / stdoutGuard 6 / host-sidecar 119（22 文件）/ node-agent-fallback 95（34 文件），与 11-21 计数一致；退出条件转由 LT-16.6 承载 |
| **LT-12** | W3 剩余域 | W1/W2 | provider 写路径/usage 网络凭据、LAN WS/公网中继、CUA PiP/Windows helper host、远程工作区、云账号 | 🟡 拆分后大半归 LT-16：`orcarouter`（阶段 1–5）与 `provider-settings`（阶段 0–4 全落，含频道 + onDidChange + 默认集）已迁；**仍缺**：CUA 权限面（已评估**不可按频道直迁**，见 `docs/specs/cua-permission-migration.md`：macOS-only + prebuilt `.app` + 托管 helper 生命周期）、远程工作区（`remoteWorkspaceService` 917 行仍 Node）、LAN supervisor 翻转；云账号云端已实现（`packages/cloud-relay`，2026-10-21 MVP），桌面侧未接 |
| **LT-13** | W1 SQLite 迁移门禁 | 无 | 基线改由 Rust 迁移生成 + `--base-ref` 只增校验 | ✅ 门禁已具备（`--base-ref` + TS/Rust 对拍）；生产者切换等 Rust 接管写库 |
| **LT-14** | LT-1 C4 唯一 PARTIAL 收尾 | 真机 | 20 任务全 PASS；随后删 Node 回退路径 | 🟡 19 PASS / 1 PARTIAL；sessions-index live upsert/remove + phase/goalStatus/title/lastAssistantPreview 已接（turn 事件驱动 + store 读尾）；**复测确认** `hasBackgroundWork`/`workflowActivity` 仍恒空（`crates/agent-server/src/sessions_index.rs:201-202` 写死 `false`/`None`，`crates/tui` 同），依赖 backgroundWorks 运行时状态迁移；任务 16 真机重连待用户环境 |
| **LT-16** | Node Host 退役专项（W6 续段） | LT-15 | `w6:readiness --fail-on-blockers` READY → 物理删除窗口 | 🟡 **进行中（本轮唯一主线）**：16.1–16.3 已落（`mode-task` 读面/编排/命令队列/snapshot/事件/写面、`window-controller` 帧订阅 + sessions-index 观察器）、16.4a 首切 + `LanHostSupervisor`（**未接 inprocess，`lan_host: None`**）、16.5 已迁 5 个频道（onboarding-record / plugin-management / mode-session / model-selection / orcarouter）+ **provider-settings 阶段 0–4 全落**、**16.4b `remote-desktop` 首切**（设置面/授权/能力探测，刻意未翻面）、**`cua-permission` 可行性评估**（结论：不可按频道直迁）；inprocess 默认集现 35 个频道；**剩余 live 面 3**：bots、remote-v3、remote workspaces，外加 remote-desktop 会话面与 cua-permission（共用 helper 准入，需 `RemoteDesktopPort` 生产实现）。详见 `2026-11-22-lt16-node-host-retirement.md` |

## LT-15 生态收口（2026-11-21，已完成）

25 提交：TUI Agent 传输（P1）、axum server-mode（P2：LAN HTTP 六端点 + `/ws/host`
WS RPC 帧桥 + capability + LAN 凭据落 credential-store + 录制）、插件进壳（P3：
记录层/卸载/安装/config 写回/git clone/zip 源）、余额查询全链路（P4：fetcher +
Personal 层凭据 + usage-stats 频道）、sessions-index 投影（P5）、W6 验收工程化 +
W2/S2 翻面执行（P6）。状态见 `2026-11-21-lt15-ecosystem-closeout.md`。
原 P2/P3/P4 的「待做」已被本轮覆盖；本清单的 LT-8/LT-9/LT-10/LT-11/LT-12 部分
条目随 LT-15 收口（详见 LT-16 拆解）。

## LT-7 本轮交付（2026-11-21）

- **打包器** `scripts/im-sdk-runtime/build.ts`：按 `packages/services/package.json` 的精确版本
  安装生产依赖树 → 确定性 tar.gz + 清单；排除 `.d.ts`/sourcemap，拒绝符号链接与原生二进制。
  真实包：589 文件、1.56 MB，lark 1.64.0 / wecom 1.0.7，入口解析正确。
- **签名** `scripts/im-sdk-runtime/sign.ts`：`generate` / `sign` / `public-key` / `verify`；
  私钥走 `MODE_IM_SDK_RUNTIME_SIGNING_KEY`，不落日志。
- **CI** `.github/workflows/im-sdk-runtime.yml`：`release: published` 或手工触发；secret 未配置
  时自动 dry-run（临时密钥、仅 workflow artifact）；清单稳定地址
  `https://github.com/eafenzhang/Mode/releases/latest/download/im-sdk-runtime-manifest.json`。
- **验证** `pnpm im-sdk-runtime:test` 3/3：打包/清单/双向验签、本地 HTTP → refresh → 激活 →
  加载入口、bundle 篡改拒绝且不切 active；另用真实 SDK 包完成一次安装与两个入口加载。
- **待做**：生成并配置发布密钥（secret + 客户端公钥）；首个 Release 真实上传；设置页版本/
  检查更新；壳资源注入 manifest URL/公钥。

## 建议执行顺序（2026-11-23 修订）

当前主线是 **LT-16**（Node Host 退役），其余长任务围绕它的门禁穿插：

1. **LT-16 续段**：~~`provider-settings` 阶段 4~~ **已落（2026-11-23）** → ~~`remote-desktop` 首切~~
   **已落（2026-11-23）** → ~~`cua-permission`~~ **评估为不可按频道直迁**（并入 16.4b 会话端口）
   → `bots`
   （前置 Rust 原生 CUA 端口）→ `bots` → `remote`/`remote-desktop`/`remote-v3` →
   16.5（LAN supervisor 单例翻转、G2 automation 真机开关、Bot 回推）→ 16.6 删除窗口。
2. **LT-11 随 LT-16.6 收口**：四组 blocker 归零后跑四仪表终值复测；不再单独排期。
3. **LT-7 收尾**（配置密钥 + 设置页 + 壳资源注入）——把已交付的在线升级真正接通，
   与 LT-16 无依赖，可穿插做。
4. **LT-8 / LT-9** 并行补尾：TUI 发送面（session/send）与 33 场景、`/ws/v1/direct` 与
   手机远控验收。
5. **LT-14** 只剩真机重连 + backgroundWorks 投影，等真机资源。
6. **LT-12 / LT-13** 按门禁穿插；LT-13 已完成，等 Rust 接管写库后切生产者。
