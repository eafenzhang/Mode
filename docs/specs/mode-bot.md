# Mode Bot 产品 spec（独立 App，对标 Grok Bot）

状态：**草案 v2（2026-10-31，待批准）**。v1（`packages/server` 无头容器 + web 管理面）未获采纳；
2026-10-31 用户决策改为**独立桌面 App**。本文依据 [product-line-naming.md](./product-line-naming.md)
规则 4：Mode Bot 实现前先补产品 spec——批准前不落任何 UI 文案与代码。

## 背景与决策记录

- 用户决策（2026-10-31）：**① 独立 App**（不做 Mode 内切换模式、不做无头服务形态）；
  **② 砍掉无用功能**（最小核心环）；**③ GUI 仿照 Grok Bot**。
- v1 的能力盘点结论仍然有效，继续作为依据：
  - Agent 运行时与服务面无 GUI 依赖：`packages/services/src/node.ts` 的
    `createLocalServices` 可在无头进程拉起；`packages/desktop/src/host-standalone/`
    是现成的 sidecar 进程入口（Tauri 壳经 stdio 长度前缀 RPC 与之通信）。
  - Tauri 壳模式可复制：`apps/desktop-tauri` 的工程结构（vite renderer +
    `build-host-sidecar.ts` + Rust crates：platform / protocol / deeplink / tray / updater）。
  - 会话 UI 可复用：`packages/ui` 的会话视图、composer、审批组件、Zustand store、
    服务客户端层。
- 缺口：teammate（命名智能体）模型、名册/活动流 App 壳、独立数据根与发行包装。

## Grok Bot 参照（行为级，clean-room 边界）

xAI 2026-08-11 early beta（[x.ai 公告](https://x.ai/news/introducing-grok-bot)）：
「always-on agent 团队」。黑盒观察到的行为与信息架构：

| 面 | Grok Bot 行为 |
| --- | --- |
| 创建流 | 给每个 bot 起名字、头衔、描述，然后录一个任务交给它 |
| 主界面 | 与 bot 的聊天线程是唯一主交互面；bot 在共享云计算机上执行 |
| 名册 | 多个命名 bot 并存（"AI teammates"），社区诉求含共享名册/各自聊天 |
| 执行可见性 | bot 在"自己的电脑"上干活，社区诉求含实时 Computer view |
| 审批 | 关键操作回报聊天流等待批准（approval handoffs） |
| 常驻 | 云端持久计算机 24/7；共享记忆、routines、skills |
| 发行 | web/iOS/Android + X + Telegram 合作；按 seat 订阅（$100–300/席/月） |

**许可与 clean-room 边界**：仿照仅限上表的**黑盒行为与信息架构**（来自公开教程与
社区评论），不抓取、不复制 xAI 的截图、图标、文案、CSS 或任何代码；所有视觉实现
使用本仓库 `DESIGN.md` 的词汇与组件。仿的是交互模式，不是像素。

## 定位与对比

**Mode Bot = 跑在用户自己电脑上的独立 App：一组命名的常驻智能体队友，各自绑定
一个本地工作目录，在聊天里派活、看它干活、批准关键操作。**

| 维度 | Grok Bot | Mode Bot |
| --- | --- | --- |
| 运行环境 | xAI 托管共享云计算机，24/7 | 用户自有电脑，App 常驻（托盘 + 可开机自启） |
| 执行体 | 云计算机内操作网页与 App | 本机真实 shell、文件、git、审批授权、验证闭环 |
| 数据主权 | xAI 云端共享 | 本地文件 + 本地加密，App 关闭即离线（差异明示） |
| 模型 | 绑定 Grok 订阅池 | 自带 key / 本地模型 |
| 协作 | named teammates + handoffs + shared memory | P0 单 teammate 独立目录；P1 例程/交接 |
| 发行 | web/iOS/Android + X + Telegram | Windows/macOS/Linux 独立桌面 App；IM 通道投递 P1 待评审 |
| 计费 | $100–300/席/月 | 开源自托管，成本 = 用户自己的模型 key |

## 产品规则

### R1 独立 App 形态

- 新 Tauri v2 应用（内部建议 `apps/bot-tauri` / `@mode/bot-tauri`，见待评审 2），
  壳模式复制 `apps/desktop-tauri`：vite renderer + host-standalone sidecar + 复用既有
  Rust crates（tray / updater / protocol），不 fork 这些 crates。
- **数据根独立**（对齐 [dev-data-dir-isolation.md](./dev-data-dir-isolation.md)）：
  与 Mode 桌面不共享任何配置/状态/凭据文件。
- 服务面复用 `createLocalServices` 全量拉起，**不为砍功能而裁 services**——砍的是
  App 的 UI 暴露面，不是服务能力；避免 host-standalone 分叉出第二套服务面。

### R2 GUI 信息架构（MVP）

```text
┌────────────────────────────────────────────────┐
│ 名册侧栏          │ 聊天主区（唯一主交互面）      │
│ ── teammate 列表  │  对话线程（复用现有会话 UI）  │
│   名字/角色/状态   │  工具过程、文件改动、终端输出  │
│   空闲/工作中/     │  作为会话流内联呈现          │
│   等待批准/异常    │  审批卡片内嵌在聊天流         │
│ ── 新建 teammate  │ 底部 composer               │
└────────────────────────────────────────────────┘
```

1. **teammate = 命名的持久智能体**：创建时录 名字 / 角色 / 描述（对齐 Grok 的
   name/title/description），并绑定一个本地工作目录——这就是它的"电脑"，
   权限不越出该目录树（工具 realpath 白名单语义沿用）。
2. **聊天即主界面**：所有交互在聊天线程完成，无第二套操作面板；
   实时执行过程直接用会话流内联呈现（MVP 不做独立 Computer view 窗格）。
3. **审批在聊天流内**：复用既有权限审批链路与卡片，不另做审批中心。
4. **名册状态可见**：空闲 / 工作中 / 等待批准 / 异常，数据源为会话运行态，
   UI 不持第二份状态。
5. **常驻**：关窗最小化到托盘、可选开机自启，任务不随窗口关闭中断；
   "App 在线 = bot 在线"（与 Grok 云端 24/7 的语义差异在产品文档明示）。

### R3 砍单清单（P0 不做）

| 砍掉 | 理由 |
| --- | --- |
| Mode Cloud / relay / 远程 workspace / 远控 | P0 纯单机；远程执行 P2 另立 |
| IM 四通道接入 UI（微信/企微/飞书/钉钉） | Grok Bot 心智是 App 内聊天；通道是投递面不是主界面。代码保留不动，P1 待评审 1 决定是否重新引入 |
| 插件商店 / skills / MCP 管理 UI | 主产品已有；本 App 用最小模型配置即可 |
| LAN 发现 / 多窗口 / 浏览器导入 / 远控设备管理 | 与核心环无关 |
| 设置面大部 | 只留：模型 provider、主题、语言、开机自启 |

### R4 teammate 的实现载体（P0 选型）

- P0 的 teammate **不走 `bot-config.v3.json`**：那是 IM 通道适配器模型（需凭据、
  通道状态机），P0 无通道，挂进去会引入半激活状态。
- 载体 = **workspace 绑定 + 持久会话 + 元数据（名字/角色/描述）**：
  workspace 沿用 `workspaceIdentity` 体系；会话沿用 session 读写分组契约；
  元数据新增一个轻量 repo 作为**唯一写方**（具体持久化位置实现时回填待评审 3）。
- P1 若接 IM，届时映射为 bot = teammate + 通道凭据，进入既有 botsService 体系。

### R5 里程碑

| 阶段 | 范围 | 判据 |
| --- | --- | --- |
| P0 | App 骨架 + teammate 创建 + 聊天闭环 + 会话流活动呈现 + 聊天流审批 + 托盘常驻 | 验收 1–5 全过 |
| P1 | 例程（automation 派发到 teammate 的目录）、IM 通道投递（若待评审 1 通过）、多 teammate 交接调研 | 另立子 spec |
| P2 | 远程执行（relay / Mode Go）、团队语义（handoffs / 共享记忆） | 另立子 spec |

## 状态所有者与时序

P0 只新增一个所有者（teammate 元数据 repo），其余全部沿用：

```text
创建 teammate：
UI 表单 ─▶ teammateRepo（元数据唯一写方：名字/角色/描述/目录绑定）
        ─▶ workspace 创建/绑定（workspaceIdentity 体系）

派活（单回合事件顺序）：
composer ─▶ 会话写入（session 写分组）─▶ ModeTaskService.createTask/sendPrompt
        ─▶ 工具流（realpath 白名单限 teammate 目录）
        ─▶ 审批请求（权限链路）─▶ 聊天流审批卡片 ─▶ 批准/拒绝
        ─▶ 任务终态 ─▶ 名册状态回到 空闲
UI 只读运行态派生名册状态点，不写任何状态。
```

## 接口

- **不新增 RPC 服务**：UI 经既有 services 客户端调用（任务、会话、审批、模型配置）。
- 新增物：teammate 元数据 repo（唯一写方）、App 壳（Tauri 工程 + 最小 renderer）。

## 验收场景

1. **核心环**：冷启动 App → 新建 teammate（名字/角色/绑定目录）→ 聊天派一个真实任务
   （改文件 + 跑命令）→ 会话流看到工具过程与文件改动 → 任务完成回复。
2. **审批**：触发审批类操作时聊天流出现批准卡片；批准后任务继续，拒绝则终止并回执。
3. **常驻**：关闭窗口缩到托盘后任务继续执行；重开窗口状态一致；退出托盘才结束进程。
4. **重启恢复**：重启 App 后 teammate 名册、会话历史、目录绑定、模型配置不丢。
5. **隔离**：两个 teammate 并行任务互不串扰（各自目录、`workspaceIdentity` 身份隔离）。
6. **回归**：`pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed` 全绿；
   Mode 主产品（桌面/web/CLI）零改动。

## 非目标

- P0 无云、无 IM、无远控、无插件商店、无多窗口、无 LAN 发现。
- 不做 Grok 式共享"云计算机"：每 teammate 绑定独立目录，不提供共享登录态/浏览器会话。
- 不复制 xAI 任何视觉资产、文案或代码。
- 不改 Mode 主产品的任何行为；不裁剪 services 面只收敛 UI 暴露面。

## 待评审

1. **IM 四通道是否 P1 重新引入**（作为 teammate 的出站/入站投递面）。建议：引入——
   代码与 spec 都已存在，是相对 Grok Bot 的差异化投递面；但主界面保持 App 内聊天。
2. **内部工程名**：`apps/bot-tauri` / `@mode/bot-tauri` 是否可用（用户可见名恒为
   「Mode Bot」，大小写按 [product-line-naming.md](./product-line-naming.md)）。
3. **teammate 元数据的持久化位置**：扩展现有会话索引 vs 独立轻量 repo；
   P0 实现时定并回填本节。

## 风险

- **会话组件复用面**：`packages/ui` 会话视图为桌面主产品设计（带文件树/多面板），
  需确认可在无外围面板的最小壳里独立工作；不能独立则拆出子组件，不开分叉副本。
- **clean-room 边界**：GUI 仿照只到行为级；评审 UI 时以"交互模式一致、视觉实现
  自有"为准。
- **常驻语义差**：本地 App 无法承诺云端 24/7（关机/休眠即离线），产品文档必须
  明示，不得宣传 24/7。
- **服务面全量拉起的资源占用**：sidecar 拉起全量 services，启动耗时与内存需在
  P0 用既有 measure 脚本量化（`measure:cold-start` / `measure:memory` 模式）。
