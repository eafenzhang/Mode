# 全量 Rust 化总计划（2026-10-29 决策：能 Rust 化的全部重写）

> **决策（2026-10-29，用户口径）**：凡能在 Rust 侧提供同等或更好行为的，一律重写；
> **工程量不作为取舍依据**，波次顺序只由依赖与风险决定。此前按「成本/收益」做的让行
> 决策（DR-IM-1 复用 TS IM、DR-TUI-1 沿用 TS TUI、DR-PM-1/DR-WEB-1 复用 sidecar）
> 一并作废，逐条更替见 §6。
>
> 本文件是 M4b 与 M5 的**上位计划**。M4b 的机制细节仍在
> `docs/plans/2026-10-29-m4b-host-into-shell.md`（线格式、路由、所有权），
> M5a 的 Agent 切换细节仍以路线图 §M5 为准，但**阶段边界按本文件重排**。

## 1. 边界：什么能重写，什么不能

**可以重写（全部纳入本计划）**：宿主服务面（`packages/services`，110,841 行 TS）、
Agent 运行时（`apps/mode-cli/packages/{core,bootstrap,contracts,adapters,dynamic-workflow,cli}`，
363,880 行 TS）、远程/Web 服务端（`packages/server` + `packages/web` 合计 17,766 行，
其中 web 的 UI 部分不在重写范围）、TUI（9,837 行）、IM 四通道、LAN/远程桌面、凭据、
SQLite、进程管理、日志、RPC 框架。共享协议类型（`packages/shared`，43,907 行）的真源
翻到 Rust `serde`，TS 侧改为生成物（与现有 `crates/protocol` 的 typeshare 方向一致）。

**不可重写（保留 web/JS 技术栈，理由不是成本）**：

| 面 | 为什么不能 Rust 化 | 终态承载 |
| --- | --- | --- |
| 渲染层 UI（React + CSS，`packages/ui`） | Tauri 用系统 WebView 渲染，UI 必须是 web 技术；改 Rust UI 等于换掉整个渲染栈（egui/leptos），属产品重做，不在本计划 | TS/React（M4 的 clean-room 替换继续） |
| 插件 / MCP / Skills **负载** | 第三方 npm/TS 代码，版权与兼容性都不允许改写 | 自家二进制 + 嵌入 JS 引擎（M5 P2 裁决），宿主 Rust、负载不动 |
| 浏览器扩展与页面注入脚本 | 必须跑在目标浏览器里 | 扩展 JS 保留；**宿主侧 CDP 客户端**可 Rust 化（可选，见 §5 W5） |
| 第三方 SDK 的对外契约 | 飞书/企微等协议要实现，但不复用其 SDK | Rust 自实现 HTTP/WS 客户端 |

**由此得到一条硬规则**：本计划完成时，核心发运集里**不再有 Node 运行时**；
唯一保留的 JS 执行面是插件负载的嵌入引擎（或按需运行时）。

## 2. 现状盘点（2026-10-29 实测，未计入测试文件）

| 面 | 位置 | 行数 | 现状所有者 |
| --- | --- | --- | --- |
| 宿主服务面 | `packages/services/src` | 110,841 | Node Host 子进程 |
| Agent 运行时 | `apps/mode-cli/packages/{core,bootstrap,contracts,adapters,dynamic-workflow,cli}` | 363,880 | Agent 子进程（`mode.cjs`） |
| TUI | `apps/mode-cli/packages/tui` | 9,837 | TS/React TUI |
| Web/远程服务端 | `packages/server/src` + `packages/web/src` 合计（web 的 UI 不在重写范围） | 17,766 | Host sidecar |
| RPC 框架 | `packages/rpc/src` | 1,310 | 两侧共用（Rust 侧已起步） |
| 频道面 | `packages/shared/src/channels.ts` | 274 个频道/事件名 | 契约真源 |
| 共享协议类型 | `packages/shared/src` | 43,907 | TS 真源（将翻到 Rust `serde`） |

宿主服务面里最大的六个域：`bots` 20,644、`mode-agent` 19,729、`session` 12,175、
`conversation-share` 5,495、`model-provider` 5,272、`git` 4,778。

## 3. 终态架构

```
┌──────────────────────────────────────────────────────────────┐
│ 壳（单个 Rust 进程，Tauri + tokio）                           │
│  ├─ 平台命令面（现有 crates/platform）                        │
│  ├─ RPC 服务面（本计划 W0–W3：原 Host 的全部频道）             │
│  ├─ Agent 运行时（W4：原 mode.cjs）                            │
│  ├─ 远程/Web 服务端（axum，W5）                                │
│  └─ 渲染层桥（WebView ←→ 进程内频道，无 stdio 帧）             │
├──────────────────────────────────────────────────────────────┤
│ 插件宿主子进程（自家二进制 + 嵌入 JS 引擎；负载是第三方 JS）    │
├──────────────────────────────────────────────────────────────┤
│ CUA helper 进程（保持独立：安全隔离是刻意设计）                 │
└──────────────────────────────────────────────────────────────┘
```

进程数从今天的 3–4 个降到 3 个（壳 / 插件宿主 / helper），且核心发运集不含 Node。

## 4. 波次（只按依赖排序，不按成本删减）

### W0 地基（进行中）

| 项 | 产出 | 状态 |
| --- | --- | --- |
| RPC 线格式 | `crates/rpc-wire` + `docs/specs/rpc-wire-format.md`，与 TS 字节级对拍 | **已完成**（`5e926cb9`） |
| 协议层 | 13 字节 `ProtocolMessage` 头、stdio 4 字节前缀、请求与回帧信封的 Rust 编解码 | **已完成**（`crates/rpc-wire::frame/stdio/envelope`，14 条黄金样例对拍） |
| 壳内 ChannelServer | 频道注册、分派、事件订阅/取消、错误映射、pending 与超时 | **已完成**（`crates/rpc-host` + `docs/specs/rpc-host-dispatch.md`，10 条行为用例） |
| 帧路由 | 按频道分流（Rust 服务面 vs 遗留 Node），启动期一次性开关 | **已完成**：判定层 + 壳内接线（`inprocess.rs`）；已接管 12 个频道，scoped 控制帧登记并按 scope 复用服务面；开关默认关闭，真机未验 |
| 启动分解度量 | `measure:host-startup`（host 四个 `startup_mark` + 解析脚本与 4 条单测） | **已完成**（标记已入 host；基线等下一次真实启动后读取） |

出口：开关为空时产品行为与今天字节级一致；打开只读频道时 UI 正常。

### W1 平台基元（先做，因为 W3 全部依赖它们）

settings、data-root、paths、credential（`ring` + 系统钥匙串）、fs/file/file-watcher、
SQLite（任务库、会话库、迁移）、日志、进程管理（spawn/kill/taskkill 语义）、storage
（磁盘用量）、system（环境/OS 信息）、media-preview。

**`memory` 频道已可接管（2026-10-29）**：`crates/project-memory` 是 `memory` 的两个
只读方法的完整实现（目录契约、符号链接拒绝、大小写精确、5 MiB 上限、快照校验），
见 `docs/specs/project-memory-service.md`，7 条用例。它取代 `setting` 成为第一个本地频道：
`setting` 有四个方法且含数据目录搬迁，留在 W3。

**已完成（2026-10-29）**：`crates/atomic-file` —— 原子写（临时文件 + rename + 退避重试 +
过期临时文件清理）与跨进程文件锁（目录 CAS + owner 文件 + 死进程/无主锁回收 + 超时），
对应 `docs/specs/atomic-file.md`，8 条用例（含 6 线程并发写不产生半截）。它是 settings /
credential / task-index 的共同前置。

**W3 已迁完整域（2026-10-29，12 个）**：`memory`（`crates/project-memory`）、
`prompt-attachment-transfer`（本地零拷贝 + 三 no-op + 不 emit 的进度订阅）、
`client-config` / `client-scenes`（审计版常量：官方服务开关恒 false，短路在凭证与网络之前）、
`media-preview`（本地 inline：扩展名/kind 校验 + 8 MiB 上限 + base64）、
`conversation-mirror`（存储 + origin 校验 + 有界化 + 适配器）、
`setting`（`get` / `update` / `updateDataBaseDir` / `ensureDefaultProject` 四方法，
含数据目录迁移）、`credential`（`load` / `save` / `delete` / `list`，AES-256-GCM +
跨进程锁 + 坏文件隔离，见 `docs/specs/credential-store.md`）、
`system`（`info` / `listIntegratedTerminalShells` / `probeIntranet`，见
`docs/specs/system-service.md`）、`file`（17 个方法：索引/搜索、文本与二进制读取、
`.modeignore` 管理，见 `docs/specs/file-service.md`）、`file-watcher`（watch/unwatch/
disposeAll + `onDynamicChange` 事件，150ms 防抖，见 `docs/specs/file-watcher-service.md`）、
`commands`（6 个方法：list / 写 / 改 / 删 / 启停 / 主目录；目录优先级、enable override、
插件命令，见 `docs/specs/commands-service.md`）。

十二个频道都已注册进壳内 `MODE_HOST_INPROCESS_CHANNELS`（默认空=纯转发），并有频道名与
TS `ServiceChannels` 的漂移守卫。**真机打开开关尚未验证**；控制帧与 `host` 频道仍归
Node Host，scoped 帧已能本地接管（按 scope 复用服务面）。剩余域的障碍：`hooks` 的
trust / config / digest 三件已就位，但 `grantWorkspaceHookTrust` 必须转发给 Agent
authority（`mode-protocol` 客户端），壳内暂无该客户端，因此频道本体推迟到 W4 与 Agent
控制面一起接；`onboarding-record` 依赖未移植的 OAuth/任务服务。各频道都经
`MODE_HOST_INPROCESS_CHANNELS` 可切到壳内，适配层契约见
`docs/specs/host-channel-adapters.md`。

`crates/workspace-identity` —— 路径归一 / 身份 key / SHA-256 前 12 位 hash（session 目录与
对话镜像目录共用），见 `docs/specs/workspace-identity.md`。

`crates/device-mid` —— 设备身份 `telemetry-state.json`（锁内生成 UUID v4、保留未知字段、
原子写回），`conversation-mirror` / onboarding / 遥测的共同前置，见
`docs/specs/device-mid.md`。

`crates/settings-store` —— `setting.json` 的路径解析（三级 home 回落）、坏文件隔离
（3×300 ms 重试后改名备份）、原子写回，对应 `docs/specs/settings-store.md`；模型层
（默认值 / `merge_persisted` / `apply_patch` / `normalize`）由 zod 生成的黄金样例对拍
（`pnpm settings:fixtures`，6 组用例）。**W1-1c 已完成（2026-10-29）**：
`updateDataBaseDir` 的完整编排——`{old}/.mode/v2` → `{new}/.mode/v2` 复制（跳过
`setting.json*`、符号链接，不覆盖目标）、Windows 安装目录禁写、`ensureDefaultProject`，
以及「先复制数据目录、再持久化指针」的顺序（`settings-store` 共 17 条用例）。
**与 TS 的已知差异**：无写队列（并发补丁不串行化）、无 `expectedAccountSettings`
账号 CAS、补丁不做 schema 类型校验（非法值逐字段回落）；`setting` 频道已接管，但开启
开关前需补齐这些差异或明确降级。

**`credential` 已完成（2026-10-29）**：`crates/credential-store` 复刻 TS 的
AES-256-GCM 密文（`enc:v1:` + base64url，三段）、`sha256(MODE_CREDENTIAL_SECRET)` 或
`mode-credential-fallback:{platform}:{homedir}:{username}` 的密钥派生（Windows 用
`GetUserNameW`、Unix 用 `getpwuid_r`，与 Node 对齐）、整文件 read-modify-write 的跨进程锁、
坏文件排他备份与「禁止自动覆盖」。`pnpm credential:fixtures` 用真实 TS provider 生成密文，
Rust 侧 6 条对拍用例全部解开；文件层 16 条、适配器 5 条。规范见
`docs/specs/credential-store.md`。TS 注释里的「后续切 Electron safeStorage/keyring」仍未做，
当前密钥随文件托管。

**`system` 已完成（2026-10-29）**：`crates/system-service` 实现三个方法——
`info`（homedir/platform）、Windows 集成终端 shell 列表（ComSpec/Git Bash，`PATH` +
`PATHEXT` 与 win32 路径语义）、内网探测（TCP + HTTP、每目标重试、marker 校验、聚合与
策略）。10 条单元测试 + 7 条探测测试 + 6 条频道测试；规范见
`docs/specs/system-service.md`。**已知差异**：连接失败文本、HTTP 客户端与重定向上限。

**hooks 前置三件已完成（2026-10-29）**：`crates/workspace-hook-trust`（trust store
严格解析，`deny_unknown_fields`，语法或 schema 任一不合法即整体 fail-closed）、
`crates/workspace-hook-config`（超时 / 输出上限 / 门禁的纯解析）、
`crates/workspace-hook-digest`（条目级与包级摘要，含 `canonicalOptional` 的
`["unset"]` / `["set", v]` 区分）。摘要与 TS `createWorkspaceHookDeclarationDigest`
有黄金字节对拍（`scripts/generate-hook-digest-fixtures.ts`）。**未做**：三个来源目录的
discovery、`saveHooks` 写路径、`grantWorkspaceHookTrust` 转 Agent authority、频道注册；
三件也还没有独立 spec（暂以 crate 文档标注的 TS 真源为准），接频道前需补齐。

**SQLite 迁移的额外约束**：迁移从 TS 翻到 Rust 时，`docs/specs/sqlite-schema-gate.md`
的基线必须改由 Rust 迁移生成，并用 `--base-ref` 证明与 TS 时代基线只增不减；否则已安装
用户升级即坏库。

出口：每个基元有 Rust 实现 + 与 TS 的行为对拍；数据目录与旧版可互换（同一份数据两边都能读）。

### W2 传输切换

把 renderer ↔ Host 的帧从「壳转发到 Node stdin」改为壳内直接服务；scoped attachment、
remote scope、流控（`saturated`/`drained`）逐项对拍。完成后 `stdioFrames`/`stdoutGuard`
从产品路径退场。

**已开始（2026-10-29）**：壳内服务面支持按 scope 复用——`scope-ready` / `scope-detach`
控制帧仍转发 Node（未移植频道的生命周期归它），同时登记进 `LocalScopeRegistry`；消费
任务为每个 scope 惰性建独立 `ChannelServer`，回帧加 `0xFF + scopeId` 前缀；Host 换代时
base 与所有 scope 一并 `dispose`。**未做**：默认全量切到壳内、`stdioFrames` /
`stdoutGuard` 退场、流控（`saturated` / `drained`）与 scoped attachment 的真机对拍。

出口：`desktop-continuous` 与 `web-remote-replayable` 两条链路语义不变；帧序无乱序；
断链/半开管道/换代行为与今天一致（复用 P1-3/P2-3/P2-4 的验收脚本）。

### W3 宿主域服务（主体）

按依赖与风险排序，每域独立完成、独立开关、独立回滚：

| 序 | 域 | TS 行数 | Rust 目标 | 关键依赖 | 进度（2026-10-29） |
| --- | --- | --- | --- | --- | --- |
| 1 | 设置 / 客户端配置 | ≈3.3k | 壳内 `settings` | W1 | `setting` / `client-config` / `client-scenes` 已接管 |
| 2 | 文件 / fs / 监视 | ≈2.8k | `ignore` + `notify` + std | W1 | `file` / `file-watcher` 已接管 |
| 3 | 任务 / 会话 / 任务索引 | ≈12.2k | `rusqlite` + tokio | W1 + SQLite 迁移 | 任务库 schema+迁移已 Rust 化（`crates/tasks-db`，`docs/specs/tasks-db.md`）；任务域（Repo / 频道 / 会话）**未**迁移 |
| 4 | 凭据 | 277 | `ring`（文件托管密钥） | W1 | `credential` 已接管（`crates/credential-store`） |
| 5 | 终端旧路径 | 1,354 | 壳已有 PTY | W2 | 未开始 |
| 6 | Git / checkpoint | ≈4.8k | `gix` 或系统 git | W1 | 未开始 |
| 7 | provider / 模型选择 / orcarouter / bigmodel | ≈7.5k | `reqwest` + `rustls` | W1 | 未开始 |
| 8 | usage-stats / coding-plan | ≈5.4k | 壳内聚合 | W1 | 未开始 |
| 9 | 远程工作区 / SSH / WSL / Docker | 远程链路 | `russh` + 本地进程 | W2 | 未开始 |
| 10 | LAN 配对 / 发现 | ≈1.5k | UDP + 壳内 HTTP | W2 | 未开始 |
| 11 | 远程桌面 | 媒体面已有 Rust helper | 壳内协调器 | W2 | 未开始 |
| 12 | 云账号 / 设备目录 / OAuth | ≈4k | `reqwest` + 本地密钥 | W1 | 未开始 |
| 13 | 对话共享 / 镜像 / 遥测 | ≈5.5k | 壳内 | W3-3 | `conversation-mirror` 已接管 |
| 14 | skills / skill-sync / mcp-sync / plugin-sync / plugins | ≈4.5k | 壳内（素材仍是 JS） | W1 | 未开始 |
| 15 | subagents / hooks / commands / memory / onboarding / broadcast | ≈5k | 壳内 | W3-3 | `memory` / `commands` 已接管；`hooks` 前置三件就绪，频道本体因 grant 依赖 Agent authority 推迟到 W4 |
| 16 | cua-permission-broker / cua-pip | ≈2.4k | 壳内 | 现有 helper | 未开始 |
| 17 | 其余（media-preview、prompt-attachment-transfer、off-peak-task…） | ≈2k | 壳内 | W1 | `media-preview` / `prompt-attachment-transfer` 已接管 |
| 18 | 系统信息 / 内网探测 | ≈0.5k | 壳内 | W1 | `system` 已接管 |

**每域完成定义**（缺一不可）：TS 实现保留为 oracle → 正常/错误/取消/事件四类对拍 →
真机用例通过 → 打开该域开关无回归 → 关闭开关仍走 TS。全部域完成后才允许删 TS 实现。

### W4 Agent 运行时 Rust 化（原 M5a，按本决策不再「按需」）

工具注册表补全 → `agent_runtime: rust | node` 双跑 → parity matrix 单真源 → 翻转 →
删除 `mode-cli` 的 TS 运行时。**硬前置不变**：对拍矩阵全绿 + 20 个真实模型端到端任务。

### W5 生态面替换

- **IM 四通道 Rust 自实现**（飞书/企微/钉钉/微信，HTTP + WS）：不再复用 SDK，
  删掉 `@larksuiteoapi/node-sdk`、`@wecom/aibot-node-sdk` 及其 protobufjs 依赖。
- **TUI 改 ratatui**（9,837 行 TS → `crates/tui`）：33 个场景清单逐项过。
- **Web/远程服务端改 axum**（17,766 行 → `crates/server`）：契约对拍 + 手机远控验收。
- **插件市场**：目录/安装/卸载逻辑进壳，素材仍按 `crate/plugin-host` 的资产层。
- 可选：浏览器自动化宿主侧 CDP 客户端 Rust 化（扩展与注入脚本保留 JS）。

### W6 收口

删 TS 宿主编排、删 `host-sidecar` 发运集、插件宿主换嵌入引擎、四仪表（语言/重合度/
体积/性能）落到终值，`T_ready` 与常驻内存按同口径复测。

## 5. 不可让步的约束

1. **clean-room**：Rust 实现只依据 spec、协议与本仓库行为；不读 `zai-org/ZCode` 及
   上游衍生实现（`docs/specs/clean-room-policy.md`）。
2. **oracle 纪律（DR-07）**：TS 运行时在删除前始终是可执行 oracle；删除前置是
   对拍矩阵全绿 + 20 个真实任务 + 性能不回退。
3. **单一所有者**：迁移期同一频道只能有一个所有者；切换是启动期决定，不做运行时热切。
4. **双链路语义**：任何 stream/snapshot/queue 改动同时验 `desktop-continuous` 与
   `web-remote-replayable`。
5. **数据兼容**：SQLite 只增、`.mode` 数据根原样复用、设置/凭据可双向读取。

## 6. 决策更替

| 原决策 | 原结论 | 2026-10-29 更替 |
| --- | --- | --- |
| DR-IM-1 | IM 由 Host sidecar 复用 TS SDK，M3 不新增 `crates/bots` | **作废**：W5 用 Rust 自实现四通道，新增 `crates/bots` |
| DR-TUI-1 | 沿用 TS/React TUI，M3 不新增 `crates/tui` | **作废**：W5 改 ratatui |
| DR-PM-1 | 插件市场由 sidecar 复用，M3 不新增 `crates/marketplace` | **作废**：市场逻辑进壳（负载仍第三方） |
| DR-WEB-1 | Web 端复用 `packages/server`，M3 不新增 `crates/server` | **作废**：W5 改 axum |
| DR-01 | 插件宿主 = Rust 核心 + Node sidecar | **保持**（负载是第三方 JS）；承载从「随包 Node」改为「嵌入 JS 引擎」 |
| DR-02 | UI 骨架期挂载、M4 clean-room 替换 | **保持**（UI 不在 Rust 化范围） |

新增 **DR-08**：能 Rust 化的全部重写，工程量不作约束；波次只按依赖与风险排序。

## 7. 「35 MB」这个口径作废

`host-sidecar/` 的 31.60 MiB（bundle ≈5.5 + externals 26.12）只是**中间状态**。
本计划终态没有 Node 宿主，因此不存在「能砍多少」的问题：
`node-pty`/`node-forge`/`undici`/`ssh2`/飞书/企微/protobufjs 全部由 Rust 实现取代，
只剩 Rust 二进制本身的增量。对外的体积口径改为「核心发运集不含 Node 运行时」。

## 8. 风险与对冲

| 风险 | 对冲 |
| --- | --- |
| 全量重写期间两套实现长期并存（双写/漂移） | 每域开关 + 单一所有者；域完成即删该域 TS；W6 一次性删编排 |
| 行为微差（事件顺序、取消、错误码、超时语义） | 每域四类对拍；错误码表进 spec；真机用例在每域出口跑 |
| SQLite 迁移翻面导致旧用户升级坏库 | 基线改由 Rust 迁移生成 + `--base-ref` 只增校验；迁移前后双向读校验 |
| Agent 运行时翻转后工具生态错位 | 工具注册表先补全；parity matrix 单真源；20 个真实任务盲跑 |
| IM 四通道自行实现与官方行为漂移 | 逐通道真账号验收；协议差异写进对应 spec；不追求与 SDK 内部实现一致 |
| 重写期间产品需求并发（0.0.x 修复）落在 TS 侧 | 每域冻结窗口；修复优先落到 Rust 实现或双实现的共享 spec |

## 9. 与既有验收记录的关系

- 0.0.92 验收后改造计划（`2026-10-27-0.0.92-acceptance-followups.md`）已收口，
  其结论作为本计划的行为基线。
- 路线图 M3 的「两台实机 LAN/RD/Web、IM 真账号、TUI 交互验收」不因重写取消，
  而是移到对应波次的出口。

## 10. 下一步（2026-10-29 续）

1. **真机验收开关**：打开 `MODE_HOST_INPROCESS_CHANNELS` 跑已接管的 12 个频道，核对回帧、
   事件与错误路径；关闭开关回归 Node 路径。启动四段耗时用 `pnpm measure:host-startup` 读取。
   验收凭据频道时用一份临时数据根，避免把真实 `credentials.json` 写坏；验收 `file` 时用
   空 workspace，注意首次扫描会创建 `.modeignore`；验收 `file-watcher` 时确认
   `onDynamicChange` 事件能到达渲染层、unwatch 后停止。
2. **W1 剩余基元**：`fs` 剩余工具与 SQLite 迁移门禁（基线改由 Rust 迁移生成 +
   `--base-ref` 只增校验）。`credential` 已落地（`crates/credential-store`，
   AES-256-GCM + 文件托管密钥；TS 注释里的 keyring 仍是后续项）。
3. **hooks 频道本体（阻塞在 W4）**：discovery、`saveHooks` 原子写可先做，但
   `grantWorkspaceHookTrust` 必须转发给 Agent authority，壳内需要 `mode-protocol` 客户端；
   与 W4 的 Agent 控制面一起接，避免壳直接写 trust store。`onboarding-record` 同样
   被注入依赖挡住（`loadUserId` 需要 OAuth、`hasExistingLocalTask` 需要任务库）。
4. **W3-3 任务 / 会话域**：体量最大（≈12.2k 行）。**任务库 schema + 迁移已 Rust 化**
   （`crates/tasks-db`，含 3 条冻结迁移、checksum 对拍、adopt/0002/0003、WAL 锁等待与
   快照函数；见 `docs/specs/tasks-db.md`），但这**不代表任务域已迁**：三个 Repo 的
   `ensureReady`、任务索引/自动化/闲时任务频道与会话链路仍在 TS。
   **门禁生产者对拍已加**（`pnpm check:tasks-schema-parity`，CI 同时跑 TS 与 Rust 快照），
   但生产者**暂不切换**：Node 仍写任务库，现在切会让门禁看不到 TS 迁移变化。等 Rust
   任务域接管写库后再把 `tasks` 快照换成 Rust 二进制并重生成基线（`--base-ref` 语义不变）。
   之后的顺序是 Repo → 频道 → 会话链路。

**本轮发现的数据根风险（未改，先记录）**：壳的 `resolve_data_root` 在未设置
`MODE_DATA_BASE_DIR` 时会读 `setting.json` 的 `dataBaseDir` 指针，而 `resolve_config`
始终把「env ?? home」作为 `MODE_DATA_BASE_DIR` 传给 Node Host，后者因而忽略该指针。
两者在「设置页改过数据目录且未设环境变量」时不指向同一棵树，所有壳内频道（含
`credential`）都会与 Node 读写不同目录。修这条要单独对齐启动路径，不在本频道范围内。
