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
| Windows XP 被控端（`apps/remote-agent`，Go，12,903 行） | XP 产物固定用 Go 1.10.8（最后一个支持 XP 的工具链）；现代 Rust 的最低 Windows 目标不含 XP | Go（手工构建、随 Release 单独上传，不在自动构建链里）。若放弃 XP，才可并入 Rust |

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

## 2.1 进度快照（2026-11-03）

| 波次 | 状态 |
| --- | --- |
| W0 地基 | **完成**：线格式、协议、`rpc-host`、壳内路由；21 个频道可启动期接管（默认关闭） |
| W1 平台基元 | **完成**：settings/paths/credential/fs/file/file-watcher/SQLite/日志/进程/system/media-preview；近两轮新增数据根（归属/迁移/初始化完整）、子进程环境策略、bundled 工具解析、网络重试状态、存储分类与占用聚合。 |
| W2 传输切换 | **部分**：壳内服务面与 scope 复用已就绪；Node Host 仍是默认所有者，`stdioFrames`/`stdoutGuard` 退场与流控对拍未做 |
| W3 宿主域 | **已接管/迁移**：设置类、文件类、凭据、git/checkpoint、conversation-mirror、skills、skill-sync、plugin-sync、subagents、settings-sync、broadcast、system、automation-service（10/10）、onboarding-record-service（9/9）、provider 旧 store 迁移、provider 余额识别/编排/目标解析、official MCP 鉴权判定、远程桌面自适应码率/画质降档状态机；**部分**：任务/会话（Repos 已迁、频道未）、provider 写路径与运行时、plugins 管理面、hooks/onboarding、usage-stats（时间范围/quota/legacy+credit monitor 已迁）、terminal（服务面与频道已迁、真机未验收）、LAN（发现纯逻辑已迁、UDP/CLI 未迁）、CUA broker（raw input socket 解析策略已迁）；**未开始**：远程工作区、云账号 |
| W4 Agent 运行时 | **协议/客户端镜像 91/115**；S0 健康快照与 S1 覆盖矩阵完成；会话级 api-retry tracker/draft registry 已迁移；频道所有权切换（S2）待决策 |
| W5 / W6 | 未开始 |

**服务层新增 crate（2026-11-01 ~ 11-03）**

| crate | 覆盖 | TS 对拍 |
| --- | --- | --- |
| `automation-service` | 10/10：create/update 调度语义 + 管理面全部方法 | UTC/Asia-Shanghai 双时区 28 用例 |
| `onboarding-record-service` | 9/9：覆盖写/匿名认领/shouldOnboard/dismiss/settings 回填/v1 升级/损坏容忍 | 7 用例 + ISO 单测 |
| `provider-legacy-migration` | 旧 provider 列表解析、migrate、端点归一化/URL 折叠、v2 模型投影、全部 v2 helper、旧 `config.json` 读取/OpenCode→v2/store 归一、文件 IO/JSON/schema 分阶段边界、Personal 层导入 | 24 条迁移 + 全部 helper + 10 条旧 config 读取 + 7 条文件边界 + 9 条 Personal 导入 |
| `provider-balance` | 6 家供应商识别/解析、fail-soft 快照编排、Settings View 目标解析 | 52 用例 |
| `official-mcp-auth` | 保留头、身份摘要、请求上下文 meta、origin 信任与 registry | 43 用例（同步 + tokio） |
| `data-root` | 归属文件、状态判定、原子写、冲突备份、旧根探测/统计/staging 清理、复制迁移、初始化器（完整） | 79 用例（真实临时目录） |
| `runtime-env` | 环境脱敏、tool-env 透传、CUA 凭据捕获、`MODE_RUNTIME_ENV`、agent 代理/CA/origin/identity、nodeEnv | 全矩阵对拍 |
| `runtime-tools` | bfs/ripgrep/ugrep 解析与 PATH、login shell bootstrap/参数/执行选项/捕获、Agent 二进制/bundle 查找、clientPlatform | 48 用例（win32 夹具） |
| `mode-api-retry` | 网络重试归一化、stream/model 事件映射、恢复进展、会话 tracker、draft registry | 58 用例 |
| `off-peak-service` | 全部服务方法：管理面、create/continue、取号/批量轮询/settle outbox/3102 续跑 | 14 条 TS 对拍 + 6 条 Rust 单元测试 |
| `storage-usage` | 存储分类目录、清理范围/保护、清理计划、扫描 job 状态/节流、占用累加/折叠、卷分组 | 97 分类 + 14 保护 + 10 清理计划 + 8 扫描 job + 3 占用 + 2 卷组 |
| `usage-stats` | App Usage / Coding Plan 时间范围、本地日期滚动、逐时/逐日坐标轴、自定义范围校验、BigModel quota limits/主额度/订阅摘要、legacy monitor 与 Coding Plan credit-usage 快照投影 | 27 范围 + 31 Coding Plan + 5 limits + 4 主额度 + 11 订阅 + 6 monitor + 5 credit |
| `integrated-terminal-shells` | Windows CMD/Git Bash 选项探测、PATHEXT/PATH 候选、win32 路径推导 | 11 用例 |
| `terminal-profile` | 终端字体栈去重/fallback、detected profile 继承、JSONC 解析、Windows Terminal/VS Code/kitty 探测、macOS Terminal/iTerm2 plist 原始探测、Alacritty TOML/YAML 探测 | 12 组合 + 9 探测 + 10 macOS + 9 Alacritty |
| `terminal-service-resolution` | 终端 shell/cwd/env/Windows PTY/ConPTY 回退纯解析 | 16 条对拍 |
| `terminal-session` | PTY 进程、输出泵、退出监视、稳定 key 重挂接、256 KiB 回放 | 7 用例（spawn/write/exit、跨块 UTF-8、重挂接、幂等 dispose） |
| `terminal-service` | profile 组合（shell/cwd/env/字体/主题/windowsPty）+ 会话编排 + data/exit 事件 | 5 用例（profile 回退、自定义字体、Windows PTY、错误串、真实 PTY 事件）；TS 完整对拍待补 |
| `remote-desktop-streaming` | 远程桌面两档自适应码率、RTT/丢帧滞回、画质降档链与冷却状态机 | 3 初始 + 9 码率序列 + 3 档位链 + 5 降档序列 |
| `zai-start-plan-billing` | Start Plan balance URL、modelIds、过期归一化、subscription/quota/remaining、plan 选择 | 3 URL + 2 modelIds + 9 过期 + subscription/quota 矩阵 |
| `provider-config-file-codec` | Personal Provider Config v1 decode/encode、旧手动规则归一化 | 16 用例（4 成功 + 12 失败） |
| `provider-config-repository` | Personal 配置 read/update、跨进程锁、私有原子写、legacy 导入/恢复降级、运行时路径解析、轮询/`onDidChange`（revision 去重、writeGeneration 防倒退）、Built-in 缓存路径与随包基线物化、Remote Release 控制文件租约/退避与刷新编排 | 34 用例（读写/导入/轮询/路径/缓存/租约/编排） |
| `lan-discovery` | Tailscale 状态 IPv4 解析与降级、单卡合并、IPv4/CGNAT 判定、定向广播目标、公告契约解析与 payload | 13 用例（解析/合并/边界/广播/公告） |
| `cua-raw-input` | raw input socket 解析策略（live → 稳定 → 按需创建 + 退避）与注入端口（事件门、broker 调用、错误码收敛） | 15 用例（优先级/开关/退避/注入） |

**2026-10-31 ~ 11-02 其他交付**

- 数据根：隔离 base 首次初始化改为原地补写归属，修复 Windows EPERM 导致 agent 退出。
- W3：`git-cli` / `git-service` / `git-checkpoint-service`、`skills-service`、`subagents-service`、
  `settings-sync-service`、`broadcast-service`、`skill-sync-service`、`plugin-sync-service`
  （7 方法含 marketplace source，频道注册为第 20 个）。
- W4：`v4/commands/query`、sessions-index topic、workspace-config 帧/订阅、
  `session/compact|goal|resume`、`syncAppRuntimePreferences`、`initialize`。
- W4 门槛：`docs/specs/host-ownership-switch.md` 设计草案；S1 矩阵
  （rust=91 / blocked=24 / todo=0，strict 门禁）；S0 只读健康快照。
- 发布与验收：v0.0.109 已发布；本地真人路径验收 19 个频道（`plugin-sync` 为后续第 20 个，
  仅 SSH/WSL 远程可达）。

**阻塞项（需决策）**：automation 归属（Host 聚合面 vs 独立频道）、`mode-agent` 切换 S2、
健康状态暴露位置、网络/凭据方法的降级语义。

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

### W0 地基（完成）

| 项 | 产出 | 状态 |
| --- | --- | --- |
| RPC 线格式 | `crates/rpc-wire` + `docs/specs/rpc-wire-format.md`，与 TS 字节级对拍 | **已完成**（`5e926cb9`） |
| 协议层 | 13 字节 `ProtocolMessage` 头、stdio 4 字节前缀、请求与回帧信封的 Rust 编解码 | **已完成**（`crates/rpc-wire::frame/stdio/envelope`，14 条黄金样例对拍） |
| 壳内 ChannelServer | 频道注册、分派、事件订阅/取消、错误映射、pending 与超时 | **已完成**（`crates/rpc-host` + `docs/specs/rpc-host-dispatch.md`，10 条行为用例） |
| 帧路由 | 按频道分流（Rust 服务面 vs 遗留 Node），启动期一次性开关 | **已完成**：判定层 + 壳内接线（`inprocess.rs`）；已接管 21 个频道，scoped 控制帧登记并按 scope 复用服务面；开关默认关闭。**2026-10-31 本地真人路径验收**：当时 19 个频道全部注册成功（`plugin-sync` 为后续第 20 个），15 个经 UI 路径验证（setting/memory/skills/subagents/commands/settings-sync/file/file-watcher/git/media-preview/credential/system + client-config/client-scenes/broadcast 间接），`prompt-attachment-transfer`、`conversation-mirror`、`skill-sync`、`git-checkpoint` 需模型或远程/手机环境，未覆盖；记录与截图在 `.plan/accept-0.0.109-inprocess/` |
| 启动分解度量 | `measure:host-startup`（host 四个 `startup_mark` + 解析脚本与 4 条单测） | **已完成**（标记已入 host；基线等下一次真实启动后读取） |

出口：开关为空时产品行为与今天字节级一致；打开只读频道时 UI 正常。

### W1 平台基元（完成；W3 的全部依赖）

settings、data-root、paths、credential（`ring` + 系统钥匙串）、fs/file/file-watcher、
SQLite（任务库、会话库、迁移）、日志、进程管理（spawn/kill/taskkill 语义）、storage
（磁盘用量）、system（环境/OS 信息）、media-preview；**子进程环境策略已迁（2026-11-01）**：
`crates/runtime-env` 覆盖脱敏键、tool-env 透传、CUA 凭据捕获、`MODE_RUNTIME_ENV` 判定，以及 agent spawn 的代理/No Proxy/CA/端点 origin/workspace identity 注入，
`pnpm runtime-env:fixtures` 对拍；**数据根归属与备份原语已迁（2026-11-02）**：`crates/data-root` 覆盖 manifest 解析、状态判定、原子写、冲突备份、旧 setting 读取、旧根探测/统计/残留 staging 清理/复制迁移/初始化器（完整迁移）；
`crates/runtime-tools` 覆盖 bundled 工具解析与 PATH 补丁、POSIX login shell 的 bootstrap/shell 选择/参数/执行选项/输出解析与注入式异步捕获、Agent 原生二进制/JS bundle 资源查找与客户端平台标识（win32 夹具）。

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

二十一个频道都已注册进壳内 `MODE_HOST_INPROCESS_CHANNELS`（默认空=纯转发），并有频道名与
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
有黄金字节对拍（`scripts/generate-hook-digest-fixtures.ts`）。**发现层已完成（2026-10-30）**：
`crates/workspace-hook-config` 补齐 strict schema 校验、候选路径发现（`.git` 祖先链、
`mode.json` / `.mode/config.json`、显式配置追加与去重）、读取错误三分类（syntax / schema / io）
与运行时根归并，与 TS 在 18 棵临时目录树上对拍；spec 见
`docs/specs/workspace-hook-config.md`。**条目解析与包级快照也已完成（2026-10-30）**：
`crates/workspace-hook-digest` 补 `resolve_entries` / `build_bundle_snapshot`（条目
`reviewItemId`、门禁位、超时优先级、按类型裁剪字段），与 TS 在 8 组纯数据用例上逐字节
对拍两个摘要；spec 见 `docs/specs/workspace-hook-digest.md`。**保存侧与读侧也已完成（2026-10-30）**：
`crates/workspace-hooks-service` 覆盖 UI Hook → 配置的写回投影、根级 enabled 归并、
按用户/工程拆分的 `saveHooks`、同目录 fsync+rename 原子写与「已审核开关」的摘要守卫，
以及读侧的三类来源投影（工程快照 / 用户 mode / legacy settings.json）、固定来源顺序的
`loadHooks`、trust store 读取与 `trustStoreCorrupt` 判定；写侧与 TS 在混合来源保存与 5 组
开关用例上对拍，读侧在 5 组来源组合上对拍（`fixtures/hooks-load.json`）；spec 见
`docs/specs/workspace-hooks-service.md`。**未做**：`grantWorkspaceHookTrust` 转
Agent authority、频道注册；
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

| 序 | 域 | TS 行数 | Rust 目标 | 关键依赖 | 进度（2026-11-03） |
| --- | --- | --- | --- | --- | --- |
| 1 | 设置 / 客户端配置 | ≈3.3k | 壳内 `settings` | W1 | `setting` / `client-config` / `client-scenes` 已接管 |
| 2 | 文件 / fs / 监视 | ≈2.8k | `ignore` + `notify` + std | W1 | `file` / `file-watcher` 已接管 |
| 3 | 任务 / 会话 / 任务索引 | ≈12.2k | `rusqlite` + tokio | W1 + SQLite 迁移 | 任务库 schema+迁移已 Rust 化（`crates/tasks-db`）；任务索引 Repo、闲时任务 Repo 与定时任务 Repo 已整体迁移（`crates/task-index-repo`、`crates/off-peak-repo`、`crates/automation-repo`，共用 `ModelSelection` 解析/编码抽至 `crates/model-selection`）；调度计算（cron 解析、`scheduleRule` 推进、carrier 校验）已 Rust 化（`crates/automation-schedule`）；Automation 运行时（20s 轮询 + manual 派发）已在 TS host-standalone 重建，写库所有者仍是 TS Repo；任务频道与会话链路**未**迁移；**闲时任务服务层已迁（2026-11-02）**：`crates/off-peak-service` 覆盖全部管理方法（含 create/continue）、取号/批量轮询/settle outbox/3102 续跑；本地生命周期 6 条 + 远程路径 8 条 TS 对拍，另有 6 条 Rust 单元测试；**Automation 服务层已迁 10/10（2026-11-01）**：`crates/automation-service` 覆盖 list/listAll/delete/setEnabled/restart/runNow/listRuns/deleteRun 与 create/update（relative delay、interval carrier、分钟锚点、cron 变更清规则、终态复活），双时区 TS 对拍（`pnpm automation-service:fixtures`）|
| 4 | 凭据 | 277 | `ring`（文件托管密钥） | W1 | `credential` 已接管（`crates/credential-store`） |
| 5 | 终端旧路径 | 1,354 | 壳已有 PTY | W2 | **shell 选项与 profile 组合已迁（2026-11-02）**：`crates/integrated-terminal-shells` 覆盖 Windows CMD/Git Bash、ComSpec/PATH/PATHEXT、固定路径与推导路径，`pnpm integrated-terminal-shells:fixtures` 11 条对拍全绿；`crates/terminal-profile` 覆盖字体栈去重/fallback、自定义字体与 detected 字号/主题继承，`pnpm terminal-profile:fixtures` 12 条对拍全绿；**terminal-profile 系统探测已迁（2026-11-03）**：JSONC 解析、Windows Terminal/VS Code/kitty raw 探测，`pnpm terminal-profile-detect:fixtures` 9 条全绿；**macOS Terminal/iTerm2 plist 原始探测已迁（2026-11-03）**：字体/字号/颜色/theme/profile 选择，`pnpm terminal-profile-macos:fixtures` 10 条全绿；**Alacritty TOML/YAML 探测已迁（2026-11-03）**：`pnpm terminal-profile-alacritty:fixtures` 9 条全绿；**terminalService 纯解析面已迁（2026-11-03）**：`crates/terminal-service-resolution` 覆盖 shell/cwd/env patch/Windows PTY/ConPTY 回退判定，`pnpm terminal-service-resolution:fixtures` 16 条全绿；**PTY 生命周期与频道已迁（2026-11-03）**：抽出 `crates/terminal-session`（portable-pty 管理器，壳命令面与频道共用）、`crates/terminal-service` 组合 profile 与 spawn/write/resize/dispose/disposeByKey 及 data/exit 事件、`crates/host-channels/src/terminal.rs` 注册为第 21 个壳内频道，规范 `docs/specs/terminal-service.md`；**剩余**：真机开关验收、node-pty 路径删除 |
| 6 | Git / checkpoint | ≈4.8k | `gix` 或系统 git | W1 | **基础层、repo 层、`git` 与 `git-checkpoint` 频道已迁（2026-10-31）**：`crates/git-cli`（命令执行/超时强杀/输出配额/local env 清理、二进制探测、status v2/numstat/diff/branch issue 解析、路径范围；39 op 对拍）；`crates/git-service`（repo 16 操作 + 门面 changes/refresh/commit；`pnpm git-service:fixtures` 7 个真实 git 仓库用例全绿，含 stagedOnly/推送/游离 HEAD）；`crates/git-checkpoint-service`（manifest store + 临时 index 隐藏 commit + diff/restore/冲突检测/删除；`pnpm git-checkpoint:fixtures` 7 个真实仓库用例全绿，含 rename 恢复、scope 边界、冲突与 force）；频道 `git`（18 方法）与 `git-checkpoint`（4 方法）已注册，`generateCommitMessage` 依赖 W4 Agent 模型 |
| 7 | provider / 模型选择 / orcarouter / bigmodel | ≈7.5k | `reqwest` + `rustls` | W1 | **已开始（2026-10-30）**：纯配置模型已 Rust 化（`crates/provider-config`，`docs/specs/provider-config.md`）——Provider/Model/ManualModel、规则容器、access/api、Restricted CEL、严格 schema 与保序序列化，`pnpm provider-config:fixtures` + 75 条解析/8 条 complete 校验对拍；**文件只读源也已迁**（`crates/provider-config-source`，`docs/specs/provider-config-source.md`）——Personal 解码/legacy manual 归一/恢复、Builtin Release 候选选择与 `mode-builtin:<rev>:<sourceKey>`、两层组合 Snapshot 与 revision，`pnpm provider-config-source:fixtures` 对拍；**账号层 Overlay 也已迁**（`crates/account-provider-overlay`，`docs/specs/account-provider-overlay.md`）——family 分组/identity 缓存/unresolvedFamilies（start-plan 例外）、API Key 折叠、off-peak 选择匹配、team scope 与 `resetPrevious`、unavailableReason、states 生成与 `account:${JSON.stringify([...])}` revision，`pnpm account-provider-overlay:fixtures` 对拍；**事实链也已迁**（`crates/provider-source-chain`，`docs/specs/provider-source-chain.md`）——文件层 + 账号 Overlay 组合、首次 fail-closed 与 last-known-good、`providerId -> config` 线格式，适配为 `agent-process::ProviderSource` 并用真实子进程 fixture 验证交付与 revision 去重；**旧 provider store 解析与迁移已迁（2026-11-01）**：`crates/provider-legacy-migration` 覆盖 legacy 列表校验、`migrateLegacyModelProviderConfig`、端点归一化/重复 URL 折叠、v2 模型条目投影，以及 v2 纯 helper（apiFormat/defaultKind/runtimeBaseUrl/modelIds/createModelConfig/reasoning 清理），`pnpm provider-legacy:fixtures` 24 条迁移 + 全部 helper 回放全绿；**旧 `config.json` 读取已迁（2026-11-02）**：同一 crate 增加 `read_mode_config_providers`，覆盖 OpenCode→v2 转换、旧 `mode` 回退、BigModel/ZAI runtime 路径纠偏与测试/生产 origin、deletedModels tombstone、store 清理与映射剥离，`pnpm provider-legacy-reader:fixtures` 10 条回放全绿；**Personal 层导入已迁（2026-11-02）**：`import_legacy_personal_provider_config` 覆盖内置模板身份、source/身份过滤、Provider/成员收集与去重、API 配置回退、context 精确规则原地替换，`pnpm provider-legacy-import:fixtures` 9 条回放全绿；**外部 Provider 余额识别/解析已迁（2026-11-01）**：`crates/provider-balance` 覆盖 6 家供应商（DeepSeek/Moonshot/SiliconFlow/StepFun/OpenRouter/Novita）的主机识别、固定查询地址、响应投影、Provider 设置视图目标解析与 fail-soft 快照编排（注入目标解析/HTTP 取回/时钟），`pnpm provider-balance:fixtures` 13+14+14+11 用例回放全绿；**ZAI Start Plan 纯逻辑已迁（2026-11-02）**：`crates/zai-start-plan-billing` 覆盖 balance URL app_version、官方 GLM modelIds 归一、plan/balance 过期过滤，以及 subscription/quota/remaining/plan 选择，`pnpm zai-start-plan-billing:fixtures` 14 条 + `pnpm zai-start-plan-quota:fixtures` 全绿；**Personal Provider Config 文件 codec 已迁（2026-11-02）**：`crates/provider-config-file-codec` 覆盖 v1 严格解码、旧手动规则归一化与规范编码，`pnpm provider-config-file:fixtures` 16 条对拍全绿；**Personal 配置仓库读写已迁（2026-11-03）**：`crates/provider-config-repository` 覆盖 read/update、跨进程锁、私有不落半截原子写、legacy 导入与无效文件恢复降级，以及 `MODE_*_PROVIDER_CONFIG_FILE` 运行时路径解析、轮询/`onDidChange`（revision 去重、writeGeneration 防倒退、poll-error 一次性上报、dispose），规范 `docs/specs/provider-config-repository.md`，11 条用例；**Built-in 缓存路径与物化已迁（2026-11-03）**：`crates/provider-config-repository` 的 `cache_paths`/`materializer` 覆盖平台与版本隔离路径、Endpoint Origin 归一与 key、随包基线严格解码/序列化/私有原子写/内容不变不重写，规范 `docs/specs/provider-config-builtin-cache.md`，5 条用例；**旧 `config.json` 文件边界已迁（2026-11-03）**：`provider-legacy-migration` 增加可注入读取的 `read_legacy_mode_config_providers`，覆盖 ENOENT→None、IO/JSON/schema 分阶段错误、顶层与 Provider 已知字段形状校验、`provider` 缺失→None，7 条用例（含 10 条既有 fixture 全量回放）；**Remote Release 控制面已迁（2026-11-03）**：`provider-config-repository` 的 `remote_sync` 覆盖控制文件模型、锁内租约/not-due/force、端点切换重置、成功间隔、失败指数退避、取消保留计数与租约不匹配保护，规范 `docs/specs/provider-config-remote-sync.md`，10 条用例；**Remote 刷新编排已迁（2026-11-03）**：同一 crate 的 `remote_refresh` 覆盖租约跳过、not-due/force、端点复核、missing、apply 结果映射与失败结算，8 条用例；**剩余**：`replace`、旧 config 嵌套模型字段/枚举 schema 完整对拍、真实 HTTP fetch/apply 与 LKG、registry/resolver（非账号部分）、model listing、真实凭据/OAuth 与 Family 可用性网络源、orcarouter/bigmodel 与频道接管 |
| 8 | usage-stats / coding-plan | ≈5.4k | 壳内聚合 | W1 | **时间范围、quota、legacy monitor 与 Coding Plan credit-usage 投影已迁（2026-11-02）**：`crates/usage-stats` 覆盖 App Usage 7d/30d/all 本地日范围、Coding Plan today/7d/30d/custom、逐时/逐日坐标轴与自定义范围错误，BigModel quota limits/主额度/订阅摘要（固定偏移日期解析），legacy monitor model/tool → Snapshot，以及 credit-usage 模型/工具序列对齐、拆分桶过滤、activity heatmap、detail/health，`pnpm usage-stats:fixtures` 27+31+5+4+11+6 与 `pnpm usage-stats-credit:fixtures` 5 条对拍全绿；**剩余**：ZAI monitor、usageStatsService 编排、缓存/网络源 |
| 9 | 远程工作区 / SSH / WSL / Docker | 远程链路 | `russh` + 本地进程 | W2 | 未开始 |
| 10 | LAN 配对 / 发现 | ≈1.5k | UDP + 壳内 HTTP | W2 | **发现纯逻辑与公告契约已迁（2026-11-03）**：`crates/lan-discovery` 覆盖 `tailscale status --json` 的 IPv4 解析与降级、同 `serverId+port` 单卡合并（物理局域网优先、tailnet 进 `extraHosts`、同 host 后者胜）、IPv4/CGNAT 判定、定向广播地址与目标去重，以及公告 magic/协议版本/字段校验与 payload 构造，13 条用例；**剩余**：UDP bind/send 收集窗口、本机地址过滤、CLI runner、最终排序、配对/HTTP |
| 11 | 远程桌面 | 媒体面已有 Rust helper | 壳内协调器 | W2 | **自适应码率/画质降档状态机已迁（2026-11-03）**：`crates/remote-desktop-streaming` 覆盖两档码率（RTT 60/30 滞回、丢帧即降、4 次健康 + 10s 驻留回升）、`sharp→balanced→smooth` 降档链、丢帧 + RTT 双阈连续 2 窗触发与 30s 冷却，`pnpm remote-desktop-streaming:fixtures` 对拍全绿；**剩余**：ping/pong 采样、媒体 WS `set-bitrate` 下发、`switchQuality` 重建会话与壳内协调器 |
| 12 | 云账号 / 设备目录 / OAuth | ≈4k | `reqwest` + 本地密钥 | W1 | 未开始 |
| 13 | 对话共享 / 镜像 / 遥测 | ≈5.5k | 壳内 | W3-3 | `conversation-mirror` 已接管 |
| 14 | skills / skill-sync / mcp-sync / plugin-sync / plugins | ≈4.5k | 壳内（素材仍是 JS） | W1 | **skills 服务面与频道已迁（2026-10-31）**：`crates/skills-service` 覆盖 `skillsService` 六方法（list / setEnabled / buildPromptContext / copyToCommon / removeFromCommon / deleteSkill），四类根扫描 + 插件技能根 + frontmatter 子集解析，`pnpm skills:fixtures` 生成 TS 真源用例、`tests/replay.rs` 逐项深比较结果与结束树（5 用例全绿，spec `docs/specs/skills-service.md`）；频道适配器已注册。**mcp-sync 本地面已迁（2026-10-31）**：`crates/mcp-sync-service` 覆盖 7 个本地方法（载入/保存/候选/远程状态/导出/写访问/导入 + filesystem 路径改写），`pnpm mcp-sync:fixtures` 8 用例回放全绿；`listWorkspaceMcpServerStatuses` 依赖 W4 Agent 控制面，频道暂不注册。**skill-sync 已迁（2026-10-31）**：`crates/skill-sync-service` 覆盖 5 个方法（候选扫描/远端状态/ustar+gzip 导出/写访问/导入），`pnpm skill-sync:fixtures` 3 用例回放全绿（含 TS 归档在 Rust 侧导入）；嵌套 `Uint8Array` 由 `rpc_wire::nested_bytes` 辅助在适配器内编解码，频道已注册（spec `docs/specs/skill-sync-service.md`）。**`skills/referenceCatalog` 协议镜像已落（2026-11-01）**：session/workspace 两种权威、条目 `enabled` 必须为 true，2 条控制面测试。**`plugin-sync` 七方法已全部迁移（2026-11-01）**：`crates/plugin-sync-service` 覆盖候选扫描/远端判重/写权限/归档导出导入/marketplace source 镜像与导入（含 TS 归档在 Rust 侧导入），`pnpm plugin-sync:fixtures` 3 用例回放全绿；频道适配器已注册（第 20 个）。真机开关验收未开始 |
| 15 | subagents / hooks / commands / memory / onboarding / broadcast | ≈5k | 壳内 | W3-3 | `memory` / `commands` / `subagents` / `broadcast` 已接管（subagents：`crates/subagents-service` 八方法 + 旧状态/旧 Markdown 迁移，`pnpm subagents:fixtures` 7 用例回放对拍；broadcast：`crates/broadcast-service` 按 Tauri 线 `parentPort: null` 的 local-only 语义实现发送 + claim 状态机，`pnpm broadcast:fixtures` 2 用例回放对拍；两者频道适配器均已注册；subagents 的 `prepareRuntimeState` 壳启动钩子未接）；`hooks` 的服务面（读侧 `loadHooks` + `saveHooks` + 已审核开关）已 Rust 化并对拍，剩 `grantWorkspaceHookTrust` 与频道本体；grant 依赖 Agent authority，整体推迟到 W4；**`onboarding-record` 服务已迁（2026-11-01）**：`crates/onboarding-record-service` 覆盖 9 个方法（覆盖写/匿名认领/shouldOnboard 判定/dismiss/settings 回填/偏好回写/v1 升级/损坏容忍），`pnpm onboarding-record:fixtures` 7 用例回放全绿；userId 与任务存在性由调用方注入，频道未注册；onboarding 流程仍被 OAuth/任务库挡住 |
| 16 | cua-permission-broker / cua-pip | ≈2.4k | 壳内 | 现有 helper | **raw input 解析与注入已迁（2026-11-03）**：`crates/cua-raw-input` 覆盖 live peek → 稳定探测 → 开关放行后单次创建 + 失败退避、probe-only 与 0 退避，以及事件数组门、socket 缺失、broker 调用与错误码收敛（unreachable → helper_unavailable、协议码保留、非法结果 fail-closed），15 条用例；**剩余**：真实 socket 发现、broker 传输、PiP session、Windows helper host |
| 17 | 其余（media-preview、prompt-attachment-transfer、off-peak-task、settings-sync…） | ≈2k | 壳内 | W1 | `media-preview` / `prompt-attachment-transfer` 已接管；**`settings-sync` 已迁（2026-10-31）**：`crates/settings-sync-service` 六方法（外部 15 agent × skills/commands/plugins + 12 MCP 来源探测与导入、Claude AGENTS.md 迁移、首启提示状态），`pnpm settings-sync:fixtures` 6 用例回放对拍，频道适配器已注册 |
| 18 | 系统信息 / 内网探测 | ≈0.5k | 壳内 | W1 | `system` 已接管 |

**每域完成定义**（缺一不可）：TS 实现保留为 oracle → 正常/错误/取消/事件四类对拍 →
真机用例通过 → 打开该域开关无回归 → 关闭开关仍走 TS。全部域完成后才允许删 TS 实现。

### W4 Agent 运行时 Rust 化（原 M5a，按本决策不再「按需」）

工具注册表补全 → `agent_runtime: rust | node` 双跑 → parity matrix 单真源 → 翻转 →
删除 `mode-cli` 的 TS 运行时。**硬前置不变**：对拍矩阵全绿 + 20 个真实模型端到端任务。

**2026-10-29 前置开工**：`crates/mode-protocol-client` 已完成协议内核——任意双向流上的
LF 分帧（只认 `\n`，U+2028/U+2029 不切帧）、请求 id 多路复用、单请求超时（默认 180s）、
通知广播、入站请求默认 `-32601`、EOF/dispose 统一失败 pending；7 条单元测试，见
`docs/specs/mode-protocol-client.md`。**进程层已落**：`crates/agent-process` 负责无控制台
拉起、stdio 接入、stderr 广播、stdin EOF 优雅关闭与整树强杀（Windows `taskkill /T /F` /
Unix 进程组 SIGKILL），5 条真实子进程测试；见 `docs/specs/agent-process.md`。
**单进程不变量已落**：`AgentRuntimeRegistry` 按 `workspaceIdentity?.trim() || workspacePath`
保证一个 workspace 只有一个 Agent，并发去重、失效重建、`reap_idle` 与 `shutdown_all`
（7 条测试）；空闲回收已按协议客户端的 pending 归零计时，并提供周期任务
`spawn_idle_reaper`（在飞请求不回收；新增 2 条测试，共 9 条）。**启动规格解析已落**：env 覆盖（command/args JSON/cwd）→ dev dist →
dev 源码 tsx → 打包 `<resources>/agent/glm/mode.cjs`，含 `--surface desktop` 改写与
cwd 回落（6 条测试）。**mode-agent 控制面第一批已落**：`AgentControlClient` 实现
`workspace/hooks/trustGrant`（workspace ref、摘要校验、严格结果解析、Agent message 原样
上抛），用 fixture 对拍请求帧并验证同 workspace 复用；并补齐 workspace 生命周期组：
existing-only `runtime_identity`（`${workspaceKey}:${generation}:${pid}`）、
`dispose_workspace`、`dispose_all`；9 条测试。**会话只读组第一片已落**：
`agent-protocol::session_read` 镜像 `modeSessionInfoSchema`/`modeSessionListParamsSchema`
（含 `Option<Option<T>>` 的 nullable-optional 三态），`AgentControlClient::list_sessions`
实现 `start-if-needed` / `existing-only` 两种运行时策略与 `MODE_AGENT_RUNTIME_UNAVAILABLE`
错误；共享夹具双端校验，4 条控制面测试。**`session/events` 已落**：复用事件信封镜像
（信封严格、载荷保持原始 JSON），游标参数与缺省省略有独立测试（3 条）。
**`session/subagents` 已落**：running/ended 严格镜像，`endedLimit` 缺省 20 且恒发（3 条）。
**`session/messages` 已落**：`agent-protocol::message` 镜像消息（user/assistant）与 13 种部件、
tool 四态；判别联合用 untagged + 分支严格，`cost` 保留 JSON 数字原样（3 条）。
**`session/read` 已落**：`agent-protocol::snapshot` 镜像完整状态快照
（settings/projection/runtime/messages + goalStats/todos/todoGroups/slashCommands），
nullable-optional 三态与浮点 `Number` 保留（3 条）。**`session/debug` 已落**：
诊断快照严格镜像，existing-only 不拉起 Agent（2 条）。会话只读组至此补齐，规范见
`docs/specs/session-read-group.md`。
**写路径第一批已落**：`session/close` 与 `setModel` / `setThoughtLevel` / `setMode`，
参数缺省省略、结果复用快照镜像（4 条）；规范见 `docs/specs/session-write-group.md`。
**`session/create` 已落**：完整建会话参数（MCP stdio/http+oauth、toolAllow/Deny、
导入历史三来源），灰度布尔只在 true 时下发，结果复用快照镜像（3 条 + 1 组 MCP/导入夹具）。
**`session/send` 已落**：完整参数（执行约束/附件/浏览器环境/Off-Peak/Bot 回推），
协议 `superRefine` 的三条约束在发出前校验，`accepted` 非 true 拒绝（3 条）。
**运行时偏好反向请求已落**：`session/requestRuntimePreferences` 的 local 权威应答
（默认值 / `-32602` / `-32603`），`RequestReply` 支持显式错误；registry 的
`set_request_handler` 对现有与后续进程统一生效（4 条）。
**会话事件订阅已落**：`session/subscribe` + `session/event` 实时流，replay 补号/排序、
按 `eventId` 去重、按 session 过滤，`Drop` 中止转发（1 条 + 双端夹具）。
**workspace 只读面已落**：`workspace/readPresentation`（closed-transport 重试一次）与
`process/childProcesses`（无 params）（3 条 + 双端夹具）。**workspace 策略写路径已落
（2026-11-01）**：`updateInteractionPreferences` / `updateModelIoPreferences` /
`updateOffPeakToolPolicy` / `updateDynamicWorkflowPolicy` 四组 params/result 严格镜像，
2 条控制面测试（含未知字段拒绝）；**广播已落**：`syncAppRuntimePreferences` 向所有活 workspace 串行下发 interaction/model IO，`-32601` 降级、其它错误上抛（3 条测试）。
**`mcp/list` 已落**：状态快照严格镜像（status/failureKind/authorization），
mode 缺省省略（3 条 + 双端夹具）。
**工作流只读面已落**：`workflows/list|get|runs`，get 的 Ok/Failure 联合、runs 的
limit 1..=50 前置校验、args 声明的三态 default（4 条 + 双端夹具）。
**工作流写侧已落**：`workflows/updateMeta|delete|move`（Ok/Failure 联合，move 的
from/to 与五种失败原因）（新增 1 条 + 3 组夹具）。
**v4 命令最小闭环已落**：`v4/command` 的 `resolveInteraction`（信封严格、ACK 六态、
rejected/stale/failed 映射 `CommandRejected`）；`commandId`/`clientId` 目前非 UUID v7，
`helloConversationV4` 与订阅仍待迁移（2 条 + 双端夹具）。
**插件只读面已落**：`plugins/list`（组件分组/hook 详情/userConfig/诊断）、
`plugins/describe`（组件清单 + metadata）与 `plugins/overview`（市场/可安装/已安装摘要 +
capability）（5 条 + 双端夹具）。**插件写侧已落（2026-11-01）**：
`plugins/setEnabled` / `restoreBuiltin` / `configure` / `resetConfig` / `validate`
五组 params/result 严格镜像（configure 的任意 JSON options、validate 的兼容性分组），
2 条控制面测试（含未知字段拒绝）。**引用目录与建议引用已落（2026-11-01）**：
`referenceCatalogWithCategory → referenceCatalog` 的 `-32601` 回退、identifiers-only
条目（subagentNames 缺省兼容）、`resolveSuggestedReference` 的可信身份 superRefine；
2 条控制面测试。**市场与安装写侧已落（2026-11-01）**：`marketplace/add|remove|update`、
`install`、`cancelOperation`、`uninstall`、`update` 七组 params/result 严格镜像，
operationProgress 通知类型已声明（订阅 facade 待所有权切换）；2 条控制面测试。
**v4 会话订阅已落**：`v4/conversation/subscribe` 参数/ACK 严格镜像，wire frame
（complete/fragment + crc32）按 subscriptionId 过滤后流入；会话帧内部先按原始 JSON
透传，随后由 `conversationDelta` 收紧（2 条 + 双端夹具）。**v4 握手已落**：host hello（能力位/profile 校验）与
clientHello 严格解析（2 条）。**v4 会话行第一批已落**：`RowBase` +
turnHeader/userInput/assistantText/reasoning，未知键按 zod strip 语义忽略，
`workflowNotification`/`display`/`amend` 暂按原始 JSON（4 条 round-trip 测试）。
**v4 会话行第二批已落**：`toolCall`（output/progress/outputPreview/cuaApp）、`artifact`、
`subagent`（2 条 round-trip 测试）。
**v4 会话行第三批已落**：`hookInvocation`（执行投影）、`timelineMarker`（9 种 payload）
与 `ConversationRow` 联合（3 条 round-trip/联合测试）。
**v4 会话快照已落**：A 区（control/availability/config/usage/queue/pendingInteractions/
backgroundWorks/subagents/goal/plan/workspaceHookAdmission）+ rows 窗口严格镜像；
`workflowRuns` 暂按原始 JSON（2 条 round-trip 测试）。
**v4 工作流运行态已落**：run/actor/node/report/pendingQuestion/concurrency/phases/artifacts
严格镜像，「零条即缺席」语义保留，快照里的 `workflowRuns` 已收紧为强类型（2 条测试）。
**存储启动门已落**：
state 严格校验、sequence/身份单调、ready/failed/首帧 30s 超时、`prepareStorage` 等待、
`getStorageStartupState` existing-only 快照与订阅、进程代次递增；9 条测试。
**Provider 交付协议侧已落**：`ProviderSource` 注入点 + 每 workspace 串行/revision 去重 +
`provider/updateAccountConfig` 与回执校验（不一致不标记、下次重试），无源时不拉起 Agent；
3 条测试。**Provider 事实链已落**：`crates/provider-source-chain` 组合文件层与账号 Overlay，
首次失败发布 fail-closed、之后保留 last-known-good；以 `providerId -> config` 对象交付，
并用本包 fixture 子进程端到端验证线格式与 revision 去重（4 + 1 条测试）。真实凭据 / OAuth /
Family 可用性网络源仍需注入实现，属于 model-provider 域。
**v4 会话增量已落**：`conversationDelta` 七个 op 强类型化（`row.appended` /
`row.upserted` / `row.removed` / `row.delta` / `state.updated` / `workflowRun.updated` /
`workflowRun.removed`），`StatePatch` 对 `modelTransition` / `goal` / `plan` /
`workspaceHookAdmission` 保留显式 null，workflow run 补丁分 header 替换 / `cleared` 键 /
`(siteId, ordinal)` 条目引用三层；订阅帧的 `payload` 与 `CompleteWireFrame.frame` 由原始
JSON 收紧为该类型，较大变体用 `Box` 控制枚举尺寸（wire 不变）。3 条夹具测试 + 订阅帧
断言（快照 / 增量 / 分片过滤）。
**v4 只读查询已落**：`conversationRowsRange`（可信 `clientMode`、可选 `beforeRowId`、
`rowId` 升序窗口 + 水位/纪元 + `hasMore`）与 `conversationPlans`（终态 ExitPlanMode 的
`toolCall` 行）强类型化，两者都支持 `StartIfNeeded` / `ExistingOnly` 两种运行时策略；
4 条控制面测试 + 双端夹具。`workflowRunEvents` 与 `workflowRuns` 枚举也已落：两者读
journal（cursor 永不失效），刻意不带 `atSeq`/`atLogEpoch`；`workflowRuns` 是重启后
投影为空时的发现面（3 条新增测试 + 双端夹具）。**用户面产物三条读面已落**：
`workflowRunArtifacts`（全量版本元数据 + itemCount/primary）、`workflowRunArtifactData`
（journal 游标条目页）、`workflowRunArtifactRead`（≤512 KiB base64 块，nullable
`nextOffset`）；协议 `superRefine` 暂未镜像（同 `workspaceHookReview` 口径）。3 条新增
测试 + 6 条双端夹具。**工作区 transcript 两条读面已落**：`workflowRunWorkspace`
（轻行清单，op/args/状态/摘要/时刻，超界截尾）与 `workflowRunNodeResult`（正文按
32 KiB 保形有界化）；2 条新增测试 + 4 条双端夹具。**文件与后台输出查询已落**：
`fileChanges` / `fileRewindPreview`（行目标 + 基线 revision/epoch）与
`backgroundBashOutput`（existing-only；`-32601` → `unsupported`、运行时不可用 →
`unavailable`）；3 条新增测试 + 6 条双端夹具。**会话用量已落**：
`v4/conversation/usage`（旧 `session/usage` 落位）的各类 token 计数与按来源输入基线
（2 条测试 + 双端夹具）。**附件面已落**：上行事务 `begin|chunk|commit|abort`、
分段读 `attachment/read` 与 `conversation/attachmentRead`、`previewSource` 的
local_path/chunked 两支、metadata-only `attachmentStat`；两条 `superRefine`（零字节
零块、target/attachmentIndex 成对）在发出前本地拒绝。4 条新增测试 + 16 条双端夹具。**连接与订阅维护已落**：`connection/flow`、
`conversation/resync`、`conversation/unsubscribe`（空结果严格、`base` 可为 null、
`forceSnapshot` 缺省省略）；2 条新增测试 + 6 条双端夹具。**应用用量已落**：
`v4/usage/stats`（summary + 热力图 + 每日/模型/工具维度，nullable 字段与单元格保留
显式 null）；2 条新增测试 + 双端夹具。**workspace-config topic 载荷已落**：
快照 + 单 op `config.updated` 增量强类型化（select/boolean 与 string|boolean 联合、
供应商分组与思考档位、`protocolVersion` 显式校验）；订阅复用 conversation subscribe，
无新 RPC 方法（3 条 round-trip 测试）。**帧与订阅已补齐（2026-11-01）**：
topic frame/wire frame、topic 解析与 `subscribe_workspace_config_v4`（与
conversation/sessions-index 共用泛型过滤），5 条测试。**`v4/commands/query` 已落（2026-11-01）**：
`CommandKey` 的 required-nullable `sessionId`（手写 Deserialize 区分缺失与 null）、
clock 探针只允许全局 key、结果 `ACK | "unknown"` 联合与 1..=64 条校验；
`AgentControlClient::commands_query_v4` 发送前本地校验，4 条协议测试 + 3 条控制面测试。
**sessions-index topic 已落（2026-11-01）**：`SessionSummary`（旧 frame 的缺席字段兼容）、
workflowActivity（runs ≤ 4、phases ≤ 32、alongside 下标界）、pending 摘要、快照/增量
（`session.upserted` / `session.removed`）与 complete/fragment wire frame；topic 前缀与
protocolVersion 校验，5 条协议测试。**订阅 facade 已接**：`subscribe_sessions_index_v4`
复用 `v4/conversation/subscribe`，按 `subscriptionId` 过滤
`v4/conversation/frame` 并反序列化为 sessions-index 帧（与 conversation 共用泛型过滤，
2 条控制面测试）。只读面到此补齐。
**未做**：native binary 兜底、真实 provider/account 网络事实、频道注册与所有权切换。
`IModeAgentService` 115 个方法已覆盖 91 个（明细见 `docs/specs/agent-method-coverage.json`）；
其余 24 个全部 `blocked`：10 个 automation（Host 聚合面归属待定）、12 个宿主事件/反向请求
（随 S2 接线）、2 个真实模型/网络方法（降级语义待定）。
所有权切换仍是 W4 下一片，`hooks` 的 grant 与任务/会话频道都等它。
**设计草案已落（2026-11-01，待批准）**：`docs/specs/host-ownership-switch.md`
定义频道级一次性切换、单 Agent 所有权不变量、请求/事件顺序、健康状态面
（不引入定时 ping）与 S0–S4 分阶段出口；开放决策包括 `not-needed` 审计方式、
`blocked` 方法降级语义、健康面暴露位置与 Host 聚合面读 agent 的方式。
**S1 覆盖矩阵工具已落（2026-11-01）**：`pnpm agent:coverage` 校验
`IModeAgentService` 方法与 `docs/specs/agent-method-coverage.json` 双向一致、
`status=rust` 的函数存在；`pnpm agent:callsites` 生成调用点证据。基线 115 个方法
（rust=91 / blocked=24 / todo=0 / not-needed=0），已接入 `verify:pre-push` 与
release verify。矩阵以 `--strict` 运行（todo 必须为 0）；automation 10 个方法与宿主事件/网络方法计入 `blocked`，待决策。
**S0 健康计算面已落（2026-11-01）**：`AgentRuntimeRegistry::health` /
`AgentControlClient::runtime_health` 返回
`{state, workspaceKey, runtimeIdentity, generation, processId, storageState, lastError}`；
传输关闭/storage 终态失败 → dead，非 storage 入口 → ready，gate 未 ready → starting；
`degraded` 待请求失败计数。3 条测试；暴露位置仍待决策 3。

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

1. **真机验收开关**：打开 `MODE_HOST_INPROCESS_CHANNELS` 跑已接管的 21 个频道，核对回帧、
   事件与错误路径；关闭开关回归 Node 路径。启动四段耗时用 `pnpm measure:host-startup` 读取。
   验收凭据频道时用一份临时数据根，避免把真实 `credentials.json` 写坏；验收 `file` 时用
   空 workspace，注意首次扫描会创建 `.modeignore`；验收 `file-watcher` 时确认
   `onDynamicChange` 事件能到达渲染层、unwatch 后停止。
2. **W1 剩余基元**：`fs` 剩余工具与 SQLite 迁移门禁（基线改由 Rust 迁移生成 +
   `--base-ref` 只增校验）。`credential` 已落地（`crates/credential-store`，
   AES-256-GCM + 文件托管密钥；TS 注释里的 keyring 仍是后续项）。
3. **hooks 频道本体（阻塞在 W4）**：discovery、`saveHooks` 原子写与读侧 `loadHooks`
   已先做完并对拍，剩 `grantWorkspaceHookTrust`——它必须转发给 Agent authority，壳内需要
   `mode-protocol` 客户端；与 W4 的 Agent 控制面一起接，避免壳直接写 trust store。
   `onboarding-record` 同样
   被注入依赖挡住（`loadUserId` 需要 OAuth、`hasExistingLocalTask` 需要任务库）。
4. **W3-3 任务 / 会话域**：体量最大（≈12.2k 行）。**任务库 schema + 迁移已 Rust 化**
   （`crates/tasks-db`，含 3 条冻结迁移、checksum 对拍、adopt/0002/0003、WAL 锁等待与
   快照函数；见 `docs/specs/tasks-db.md`）。**任务索引 Repo 已整体迁完**
   （`crates/task-index-repo`：`ensure_ready/close`、meta CRUD、状态写入、列表/搜索、
   归档清理、分组视图与分组管理、`syncTaskMetaAtGroupedTop`；与真实 TS
   `TaskIndexRepo` 逐步骤对拍（主场景 109 步 + 独立空库 8 步，含 bootstrap 表快照）；
   见 `docs/specs/task-index-repo.md`）。
   闲时任务 Repo 也已迁（`crates/off-peak-repo`：状态机守卫、认领回收、核销 outbox，
   50 步对拍；见 `docs/specs/off-peak-repo.md`）。**定时任务 Repo 已整体迁完**
   （`crates/automation-repo`：37 个公开方法、创建上限、截止结算、认领回收、
   30s→15min 退避与 5 次放弃、manual run 生命周期、运行历史与清理；
   与真实 TS `AutomationRepo` 70 步逐步对拍；见 `docs/specs/automation-repo.md`）。
   三个 Repo 共用的 `ModelSelection` 严格解析与落盘编码已抽成 `crates/model-selection`
   （off-peak-repo 改用共享实现，删除本地副本）。
   **调度计算已迁（2026-10-30）**：`crates/automation-schedule` 覆盖 cron 解析与
   下次/上次触发（对齐 croner 10，含 `L`/`W`/`#`/名称/6-7 段/昵称）、`scheduleRule`
   六种 unit 的推进、相对延迟与 carrier 归一化、一次性错过窗口；242 条 cron 表达式 ×
   多个基准时刻、19 条规则 × 5 个时刻，UTC 与 `+08:00` 双偏移逐步对拍；
   见 `docs/specs/automation-schedule.md`。夏令时跃迁与 croner 的 `+` 前缀/Quartz 周编号
   未纳入（spec §7）。
   **TS 侧 Automation 运行时已重建（2026-10-24，`8f2d024d`）**：20s 轮询
   `claimDue`/`claimManualRuns`、manual 派发与终态结算在 host-standalone 进程内运行
   （`packages/services/src/session/automationRuntime.ts`，契约见
   `docs/specs/automation-runtime.md`）。该运行时仍以 TS `AutomationRepo` 为写库所有者，
   Rust `automation-repo` 要等任务/会话频道接线后随运行时一起翻面。
   **随 Electron 线删除且未恢复的两条链**（记录为该域收口缺口）：闲时任务派发链
   （spec 已声明为独立后续工作，`OffPeakTaskRepo` 只有 Repo 没有 runtime）与
   keep-awake（Electron `powerSaveBlocker` 在 Tauri 线无对应接线）。
   仍未迁：任务频道与会话链路。
   **门禁生产者对拍已加**（`pnpm check:tasks-schema-parity`，CI 同时跑 TS 与 Rust 快照），
   但生产者**暂不切换**：Node 仍写任务库，现在切会让门禁看不到 TS 迁移变化。等 Rust
   任务域接管写库后再把 `tasks` 快照换成 Rust 二进制并重生成基线（`--base-ref` 语义不变）。
   之后的顺序是任务频道接线 → 会话链路。**两者都被 W4 挡住的结论（2026-10-30）**：
   `mode-task` 的实现是 5.7k 行 `modeTaskServiceAdapter`，绝大部分方法是对
   `IModeAgentService` 的投影（createTask/sendPrompt/snapshot/流式事件）；`mode-session`
   同样以 `agentService` 为基础。壳内频道是整频道切换、不做「半个频道」，而 Rust 侧
   还没有到 Node 频道的 RPC 客户端，所以这两条链必须等 W4 的 Agent 控制面就位后再接，
   不能先从索引/列表方法入手。

**数据根风险（2026-10-31 已修）**：壳的 `resolve_data_root` 在未设置
`MODE_DATA_BASE_DIR` 时会读 `setting.json` 的 `dataBaseDir` 指针，而此前的 `resolve_config`
始终把「env ?? home」作为 `MODE_DATA_BASE_DIR` 传给 Node Host，后者因而忽略该指针。
两者在「设置页改过数据目录且未设环境变量」时不指向同一棵树，所有壳内频道（含
`credential`）都会与 Node 读写不同目录。现在壳在启动 Host 前用同一份解析
（env → 指针 → home，`platform::commands::logs::resolve_data_base_dir`，已有单测）
算出基目录并透传，壳内频道与 Host 同树；`host-shell.log` 路径也复用这一份。

**2026-10-30 工作区清理（knip 收口，待提交）**：展开 knip 全仓覆盖后删除死代码——
`packages/formal-proof`（形式化对拍实验包，`product-projection` / `projection-state`
的注释引用同步摘除）、`apps/mode-cli/packages/debug`（开发调试面板）、
`node-repl-host` 陈旧 dist-types、M4 换装前被替换的旧 UI primitives
（`packages/ui/src/components/ui/*`、`ai-elements/*` 及配套字体）、fa-IR 语言包
（locale 收敛为 en-US / zh-CN）；未用依赖随 `pnpm-lock.yaml` 一并清出。
`docs/specs/m4-ui-foundation.md` 同步删除从未接入产品路径的 P1.2 参考骨架
（`AppShell` / `WorkspaceFrame` / `usePanelResize` 等），布局要求改以
`WorkspaceShellLayout` 为准。删除面不触及 §2 计数口径中的 Agent 运行时六包与
services 主体（services 仅去 3 个无引用文件），现状盘点数量级不变。

**2026-10-31 knip 残留收口**：`knip:gate` 在展开覆盖后还报三处——根脚本
`generate-automation-schedule-fixtures.ts` / `generate-provider-config-fixtures.ts`
隐式依赖 `croner` / `zod`（已补进根 `devDependencies` 并更新 lockfile），
`packages/cloud-relay/src/cloudCrypto.ts` 无引用（实际在用的是 `secretBox.ts`，
已删除）。`pnpm verify:pre-push` 恢复全绿。
