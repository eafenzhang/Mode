# M3 生态层与「Tauri 版可发布」Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `subagent-driven-development`（推荐）或
> `executing-plans` 逐任务实现。步骤用 `- [ ]` 复选框跟踪。
>
> **本文件的分层，开工前先读**：Task 0 是**逐步骤可执行**的（含真实命令与判据）；
> Task 1–5 是**步骤级大纲 + 接口**。理由见文末「为什么 Task 1–5 只到步骤级」——
> 它们的细节取决于 Task 0 的实测结果，现在写满代码块等于产出会作废的计划（M2 计划
> 用同一条理由，见 `docs/plans/2026-10-16-tauri-agent-runtime.md` 文末）。
>
> **状态（2026-10-18 巡检回填）**：Task 0–5 的**实现提交均已落地**——
> Task 0 `0a21e34b`（修复轮 `76d7a417`）、Task 1 `62dc1a61`（`05fe2b96`）、
> Task 2 `ee46dea3`（`c4f7af7e`）、Task 3 `057b7e9d`（`76d51424`）、
> Task 4 `eb5fa592`（`e18ca4a5`、`be1fc1c8`）、Task 5 `50586571`（`4aa30270`）。
> 本文各任务的 `- [ ]` 复选框**未逐条回填**（执行记录以提交与 `docs/specs/tauri-renderer-host.md` 为准）；
> 阶段验收是否全部满足仍需逐条复核，尤其 macOS / Linux 出包腿仍按文末「范围收窄登记」延后。
> 路线图 M3 节里的**生态域**（插件市场 / IM bot / LAN / 远程 / Web / TUI）尚未开始，另行立项。

**Goal:** 把存量 renderer 挂到 Tauri 壳上，补齐 M1 登记给 M3 的平台面缺口，并把 **Windows 出包**与
体积验证到位（macOS / Linux 两腿在本阶段**延后**，理由与代价见文末「范围收窄登记」）——使 Tauri 版从「命令面验收台」变成**可发布的产品**，从而让 M5 有能力把发布线
从旧 Electron 线切过来。

**Architecture:** 渲染层不直接说话，它等待一个 **Host** 经 `MessagePort` 递上
`IServiceAccessor` 与一条 database-startup 状态流；今天的 Host 是 Electron `utilityProcess`
里跑的 `packages/desktop/src/host/index.js`。取证结论（见 DR-M3-1）是**这条 Host 路径几乎与
Electron 无关**，因此 M3 的做法是：用一个 Node **sidecar** 承接同一份 Host/ services 代码，
由 Tauri 壳负责把它 spawn 起来并把端口送进 webview；Electron main / preload 的位置由 Rust 壳
与 Tauri 的原生能力替代。

**Tech Stack:** Rust 1.99（`stable`）、tauri 2（含 `tauri-plugin-shell` 或自定义 command 做进程
管理）、Node 24（sidecar 运行时）、`packages/rpc`（`ChannelServer`/`ChannelClient`）、
`packages/services`（`IServiceAccessor` 的 45 个服务）。测试侧复用 `tsx --test` 与 M2 Task 0 的
oracle 驱动思路。

**Spec:** `docs/plans/2026-10-09-tauri-rewrite-roadmap.md` 的 M3 节（阶段边界与验收）；
已交付的平台面口径在 `docs/specs/tauri-platform-commands.md`（§4.2 是「renderer 为什么挂不上」
的证据表，§5 是登记给 M3 的缺口清单）。本阶段实现 spec 于 **Task 0 之后**产出：
`docs/specs/tauri-renderer-host.md`（Host 契约与端口传输口径）。

## Global Constraints

- **clean-room**：`crates/**`、`apps/desktop-tauri/**` 全部适用；**只读**上游衍生文件的接口面
  （导出签名、类型声明、协议形状、测试断言的输入输出），**禁读实现体**。上游衍生的判定口径是
  「该文件在 `upstream/zcode-baseline` 里存在」。
  〔**待核实**：`docs/specs/clean-room-policy.md` 写基线 = `29628c9`，而
  `git rev-parse upstream/zcode-baseline` 实测是 `aac4755666d09fdcd70272fcf063c077a639015f`。
  开工前先确认哪个是权威，否则接口面判定可能整体错位。这项差异记在本计划里而不是顺手改 spec——
  它是既有文档问题，与 M3 无关但会影响 M3 的纪律执行。〕
- **接口面的判定**：**一个函数的实现体（function body）不算接口面**。只读「导出签名、类型声明、
  协议形状、测试断言的输入输出」；确需读某段实体时，必须在任务报告里写明**文件 + 行号范围 +
  为什么非读不可**（M3 Task 0 的 spike 文档 §9 是这样登记的样本）。
- **既有语义不得丢失**：`workspaceIdentity` 贯穿、每窗口一个 window-scoped Local Host、
  远程 workspace 由窗口内连接注册表管理（`AGENTS.md` 的「进程、协议与远程控制」）。
  renderer 挂载不得绕过 Host 的 owner/lease 语义。
- **不做的面**：M2 的运行时 Rust 化（另一条线，与 M3 并行）、M4 的 UI clean-room 替换（M3 挂的是
  **存量** renderer）、M5 的发布线切换（本计划只把「可发布」做成前提）。
- **语言政策**：新增只允许 Rust / TS / HTML / CSS。新脚本用 `.ts`、新测试用 `.mts`
  （2026-10-16 有一次提交因新增 `.mjs` 被语言政策门禁判红）。
- **每个任务结束**：`node scripts/rust-cargo.ts clippy --workspace --all-targets -- -D warnings`
  零告警、可跑用例全绿、`pnpm verify:pre-push` 通过（它含 lint + 语言政策 + 架构检查）。
- **本机 Rust**：一律经 `node scripts/rust-cargo.ts …`（本机无 VS Build Tools）。

---

## 决策记录（开工前先读；DR-M3-1 已由人确认，2026-10-16）

取证范围：`packages/desktop/src/{renderer,preload,main,host}/**`、`packages/rpc/src/**`、
`packages/shared/src/{channels,database-startup}.ts`、`packages/services/src/accessor.ts`。
除接口面外不读实现体。

### DR-M3-1：Host 用 Node sidecar（**这条修订了 M1 的「不带 Node 运行时」口径**）

**事实**（取证，2026-10-16）：

- 渲染层需要**两条通道**，都经 `window.postMessage`（`MessagePort` 过不了 `contextBridge`）：
  1. 一个被转移的 `MessagePort`，承载 `packages/rpc` 的 `ChannelServer`，暴露
     `IServiceAccessor`（`packages/services/src/accessor.ts` 的 ~45 个 readonly 服务）；
  2. 一条 database-startup 状态流：渲染层在 `state.phase === "ready"` **且** 端口的
     `databaseStartupId` 与 `state.startupId` 匹配前不挂业务根；30 秒内没有任何状态就画失败屏
     （`errorCode: "startup_status_timeout"`）。
- 今天的 Host 是 **Electron `utilityProcess` fork 出来的**
  `packages/desktop/src/host/index.js`；**main** 建 `MessageChannelMain`、把 `port2` 递给 Host、
  `port1` 递给 renderer 的 `webContents`；**preload** 再把端口用 `window.postMessage` 转交给页面。
- **Host 路径几乎与 Electron 无关**：`packages/desktop/src/host/**` 里只有一处 electron import
  （`host/electronPort.ts` 的 `MessagePortMain` 包装）；`packages/services/**` 与
  `packages/client/**` **零** electron import。Electron 专有的部分集中在 main 与 preload——
  而这两处正是 Tauri 壳要替代的。
- 可替代性：`process.parentPort`（utilityProcess IPC）→ `child_process.fork` 的 IPC；Node 的
  `worker_threads.MessagePort` 本身就满足 `packages/rpc/src/protocol.ts` 的 `MessagePortLike`
  形状（`addEventListener/removeEventListener/postMessage/start/close`）。

**决策**：Host 用 **Node sidecar**，承接存量 Host + `packages/services` 代码。不把 services
层重写成 Rust。

**依据**：Host 要跑的是存量 TS services 层，其中含 `node:sqlite` 与 `node-pty` 这类原生依赖；
用 Rust 重写这一层等于把工作量在 M2/M3 之上再翻一倍，而且**没有对拍资源**（旧实现是 TS，
Rust 侧要重新建立 oracle）。DR-01 已经把「Node sidecar」定为插件宿主的方向，这里是把同一决定
用在 Host 上，方向一致。

**代价（若判错）**：产品将携带 **Node 运行时**，与 M1 验收写的「安装包内**不带任何 Node 运行时**」
（`docs/specs/tauri-platform-commands.md` §4 末、路线图 M1 验收）**直接冲突**。

**结论（人已于 2026-10-16 确认）**：判据是 **「哪种快就先上哪种」**——Node sidecar 胜出，因为它
复用存量 TypeScript services 层；把 Host/services 层用 Rust 重写要多写约 45 个服务，而且**没有对拍
资源**（旧实现是 TS，Rust 侧要重新建立 oracle），慢一个数量级。

**确认后的口径**：**M1 那条口径对「产品阶段」修订**——它描述的是 M1 骨架（纯静态资源）的真实
状态，成立；但真实 UI 必须由 Host 提供 `IServiceAccessor` 才能挂载，所以产品阶段必然带 Node。
替代口径为 **M3 的体积门禁**：核心安装包 ≤30MB（沿用 M3 既有验收），并在 Task 5 实测。
> **〔2026-10-16 后续更正（Ruling 26）〕** 上面这句「核心 ≤30MB」**已被实测推翻**，不是待办：
> Task 2 量出 sidecar 载荷 96.79 MiB（真实传递闭包 ≥102.61 MiB），而那台机器上 `node.exe`
> 光 gzip 就 33.91 MiB——**仅它一项就已超过 30MB**，剪掉跨平台 prebuilds 压缩后也只省约 6.6 MiB。
> 因此产品阶段的体积门禁已**拆成两份分别门禁的预算**（(a) 核心 = 壳 + UI 资产 + Host bundle；
> (b) Node 运行时 + sidecar externals），算术与偏差登记见本文档的「体积门禁拆分登记」一节。
> **这里留原文不动是为了让权威链可追**：DR-M3-1 当初就是这么写的，改动记在后续裁决里，而不是
> 回头改写决策记录（同 Ruling 20 对路线图的处理）。要守住 ≤30MB 的唯一出路是把 Host/services
> 层用 Rust 重写——即本条里被「快」字否掉的那条慢路径。
**代价已被接受**：产品携带 Node 运行时（`docs/specs/tauri-platform-commands.md` §4 第 6 条已按此口径
加注）。

**未采取的选项（保留记录）**：把 Host/services 层也用 Rust 实现——那是比本计划大一个量级的改动
（等于新增一个 M2 量级的阶段）。这条岔路口记在这里而不是删掉：**若日后判定「产品不得带 Node」
的代价不可接受，要重开的就是这一处。**

### DR-M3-2：Host sidecar **随包发布**，插件/MCP/Skills 的负载仍按需下载

DR-01 只说了插件宿主「首启按需下载、校验 sha256、健康探测」。但 Host 是**UI 能渲染的前提**：
若 Host 也按需下载，全新安装 + 断网 = 失败屏，这与 M3 验收的「断网时核心功能全在」直接冲突。

**决策（2026-10-16 确认，判据同样取「哪种快就先上哪种」）**：Host sidecar（含其 Node 运行时）
**随安装包发布**，**不做** CDN 下载 / sha256 校验 / 首启健康探测那一套——按需下载要多出一套
基础设施与首启 UX，而 Host 是**UI 能渲染的前提**，不能依赖网络。插件/MCP/Skills 宿主负载保持
DR-01 的按需下载。两者是同一份 Node 运行时还是两份，由 Task 1 按体积实测决定（一份更省体积，
但会让「插件宿主可独立升级」变复杂）。

**代价（若判错）**：安装包体积上升（Node 运行时本身在历史证据里是 ~50MB 量级：
`scripts/fnos-fpk/build-lib.ts` 对捆绑 `node` 有 `>50MB` 的异常断言；而
`packages/desktop/scripts/prepare-agent-node-bundle.mjs` 记着「用 Electron 自带的 Node 执行，
单平台体积从 ~180MB 降到 ~16MB」）。**当前没有 Host 闭包的体积测量**，Task 5 必须实测。

### DR-M3-3：**端口如何抵达 Tauri webview 是未验证项**——它是 Task 0 的全部内容

Electron 有一条 Tauri 没有的路径：preload 里 `contextBridge` + `ipcRenderer` 把端口转交给页面
（`packages/desktop/src/preload/index.ts`）。Tauri **没有 preload/contextBridge 等价物**，
仓库里**没有任何证据**说明 `MessagePort`（或等价物）怎么进 webview。

**决策**：不先猜。Task 0 用 spike 拿证据，候选路线按可行性排序：

1. **Tauri 初始化脚本 + 自定义 command 转移端口**：若 Tauri 2 能在页面加载前注入脚本且允许从
   Rust 侧转移一个可 `postMessage` 的端口，则可复用渲染层现有代码（**首选**，因为渲染层不用改）；
2. **壳内桥 + 渲染层连接层适配**：壳起一个本地桥（WebSocket 或 Tauri IPC channel），用一个满足
   `MessagePortLike` 的适配器接进 `connectViaMessagePort`。**注意**：渲染层是上游衍生文件，
   改它要守 clean-room（只读接口面、不照抄实现），且这条会影响到 M4 的替换计划；
3. **`worker_threads.MessagePort` + 自定义转移**：若 (1)(2) 都不通，评估把端口换成可序列化的
   RPC 通道（需要 `packages/rpc` 侧提供一种非端口传输——属**新增接口**，要单独评估）。

**判据（Task 0 的验收）**：在 Tauri 壳里跑通「注入 → 转移 → 页面拿到对象 → 该对象能承载
`ChannelServer` 的一次 `Initialize` 往返」，并且**不修改 renderer 的启动链**（若必须修改，
把修改点与理由写成 Task 0 的产出与决策记录，而不是悄悄改）。

---

## 与 M1 缺口清单的对应（`tauri-platform-commands.md` §5）

| §5 登记项 | 本计划归属 |
| --- | --- |
| 存量 renderer 的挂载 | Task 1–3（本阶段主线） |
| `getApplicationIcon` 的原生图标解码 | Task 4（Windows GDI；macOS `NSWorkspace` 半边**未实现且本阶段不验**——本阶段没有在范围内的 macOS 构建，写了也验不了） |
| `saveFile` 的 `sourceUrl` 分支 | Task 4（需要一个带超时/大小上限的 HTTP 取回面） |
| `onOAuthCallback`/`onPaymentCallback` 之外的订阅型方法 | Task 4（依赖 Host 的事件源，Task 1–3 打通后才有） |
| 深链的单实例转发 | Task 4（窗口模型：Windows 上点链接不能再起新进程） |
| Tauri 产物进 Release | **不在本计划**：M5 |
| 更新链的实际可用（签名密钥、`latest.json`、三平台演练） | **不在本计划**：M5；三平台演练在 M5 之前**由本阶段的延后决定顺延**（见「范围收窄登记」） |

---

### Task 0: 端口传输 spike（本阶段的地基）

**为什么它是 Task 0**：DR-M3-3 的三种路线决定后面每一个任务的形状——决定渲染层要不要改、
Host 的传输实现怎么写、甚至 M4 的替换计划要不要跟着动。没有这个证据就写 Task 1，等于凭印象
挑一条路线，而这类「传输层猜错」的返工成本最高（要动渲染层启动链与 rpc 传输）。

**Files:**
- Create: `docs/plans/2026-10-16-m3-port-transport-spike.md`（spike 结论与决策记录）
- Create: `apps/desktop-tauri/experiments/port-transport/`（一次性验证代码，**不入交付**；结论落文档后删除或移入 tests）
- Test: 一次性脚本即可，不进 CI（spike 的产物是**结论**，不是可持续的用例）

**Interfaces:**
- Consumes: M1 的壳（`apps/desktop-tauri/src-tauri`，已能开窗并渲染前端）、
  `packages/rpc` 的 `ChannelServer`/`ChannelClient` 与 `MessagePortLike`。
- Produces（写进 spike 文档，供 Task 1 引用）：
  - 选定路线的**确切机制**（Tauri 2 的哪个 API、哪一侧建端口、如何转移）；
  - 渲染层**是否需要改**，若需要，改动点与最小改动面；
  - 一条可复现的验证命令与它的实测输出。

- [ ] **Step 1: 读接口面，写下 spike 要回答的三个问题**

只读接口面：`packages/rpc/src/protocol.ts` 的 `MessagePortLike`/`MessagePortProtocol`、
`packages/rpc/src/channelServer.ts` 的 `ChannelServer` 构造函数与 `ready()`、
`packages/client/src/messageport.ts` 的 `connectViaMessagePort(port)` 签名。
把三个问题写进 spike 文档：**(a)** Tauri 2 里有没有「页面加载前注入、且能拿到可 `postMessage`
的端口对象」的能力？**(b)** 若没有，能否把一个等价对象注入页面而不改渲染层的
`connectViaMessagePort(port)` 调用点？**(c)** 注入对象的 `addEventListener/postMessage` 语义
能否满足 `ChannelServer` 的 `Initialize` 往返？

- [ ] **Step 2: 路线 (1) 的实测——Tauri 初始化脚本 + 端口转移**

在 `apps/desktop-tauri/experiments/port-transport/` 里做最小验证：壳侧建端口、页面加载前注入
一个能 `postMessage` 的对象、页面侧用它做一次往返打印结果。
Run: `node scripts/rust-cargo.ts build --release -p mode-tauri` 后启动该实验壳。
Expected: 记录**具体**结果——可以是「可行（附 API 名与最小代码）」或「不可行（附报错原文）」。

- [ ] **Step 3: 路线 (2) 的实测——壳内桥 + `MessagePortLike` 适配器**

若 Step 2 不可行：壳起一个本地桥（Tauri IPC channel 或 loopback WebSocket），页面侧用一个满足
`MessagePortLike` 的适配器包住它，再喂给 `connectViaMessagePort`。
Expected: 同上，记录可行性与**渲染层是否必须改动**。

- [ ] **Step 4: 路线 (3) 的评估——为什么必须新增传输**

只有在 (1)(2) 都不可行时才走。写明：需要 `packages/rpc` 提供什么新接口、谁消费、
以及它对 M4 替换计划的影响。**若走到这一步，停下来向人确认**——这已超出「把存量 renderer 挂上」
的范围，属新增接口设计。

- [ ] **Step 5: 收口 spike 文档并提交**

`docs/plans/2026-10-16-m3-port-transport-spike.md` 至少含：结论（选了哪条路线及为什么）、
证据（命令 + 原始输出 + 关键 API 名）、**渲染层是否需要改**、以及「Task 1 可以据此开始」的明确
一句。

**复现面必须提交**（2026-10-16 修订，原「不留一次性脚本」口径已作废）：spike 的全部价值在于
「它证明过某件事」，所以**复现就是产出**——harness 以最小且有文档的形态留在 `apps/desktop-tauri/`
下、由测试拥有（本次落地：`test/harness/port-transport/` + `test/fixtures/`，Rust 侧 bin 用
`required-features` 门住，默认构建/`tauri build`/`clippy --all-targets` 都不带它）。
只有**当已提交的自动化测试覆盖了同一条路径**时，才可以删掉对应的手工 harness；反过来，删掉
复现面只留一段结论，等于把证据降格为不可复核的断言——**这是本计划第一版的原话，此处更正。**

**在文档里写清覆盖分工**：哪些路径由自动化测试守着、哪些只能人工复现（附确切命令与判据）。

```bash
git add docs/plans/2026-10-16-m3-port-transport-spike.md apps/desktop-tauri
git commit -m "spike(tauri): M3 Task 0——MessagePort 抵达 Tauri webview 的可行路线与证据"
```

---

### Task 1: 最小 Host sidecar（骨架 + 控制通道）

**Files:**
- Create: `crates/sidecar-host/`（Rust 侧：spawn、监督、传输端口、退出清理）
- Create: `packages/desktop/src/host-standalone/`（Node 侧入口：在无 Electron 环境下把
  `packages/desktop/src/host` 跑起来）
- Test: `crates/sidecar-host/tests/`、`packages/desktop/tests/host-standalone/`

**Interfaces:**
- Consumes: Task 0 的传输路线。
- Produces: `sidecar_host::spawn(app) -> HostHandle`（含 `port()` 取端口、`shutdown()`）；
  一个能对外暴露**空实现**（每个服务都返回 `not_implemented`）的 `IServiceAccessor`——
  先让渲染层的启动链跑通，再逐个换真实现（Task 2）。

**- [ ] Step 1–6: 大纲**（子计划在 Task 0 之后出，理由见文末）
必须覆盖：sidecar 的生命周期（启动/健康探测/崩溃重启策略/退出清理不留孤儿）；
`process.parentPort` → `child_process.fork` IPC 的等价替换（控制消息形状取自
`packages/shared/src/channels.ts` 的 `HostMessageTypes`/`HostResponseTypes`）；
端口经 Task 0 选定的路线送进 webview；**渲染层在 30s 内看到 `phase: "ready"`**。

---

### Task 2: 真服务面 + database-startup 通道

**Files:**
- Modify: `packages/desktop/src/host-standalone/`（接真 `packages/services`）
- Create: `packages/desktop/src/host-standalone/databaseStartup.ts`（把 `startup/storageState`
  流接到渲染层要的 `DatabaseStartupState`）

**Interfaces:**
- Consumes: Task 1 的 `HostHandle`、`IServiceAccessor`。
- Produces: 真实 `IServiceAccessor`（45 个服务）+ 满足 30s 就绪判定的 startup 状态流。

**- [ ] Step 1–6: 大纲**
必须覆盖：`node:sqlite` 与 `node-pty` 在 sidecar 里的可用性（原生 ABI）；
数据根隔离（沿用 M2 的 `MODE_DATA_BASE_DIR` + `HOME`/`USERPROFILE` 双覆写口径，**对拍/测试绝不
碰用户真实 `~/.mode`**）；启动失败时必须把 `phase: "failed"` + `errorCode` 送到渲染层，
而不是静默挂起。

---

### Task 3: 把存量 renderer 挂上 Tauri 壳

**Files:**
- Modify: `apps/desktop-tauri/frontend/`（从命令面验收台切到真实 renderer 产物）
- Modify: `apps/desktop-tauri/src-tauri/tauri.conf.json`（`frontendDist` 指向真实 renderer 构建产物）

**- [ ] Step 1–5: 大纲**
必须覆盖：renderer 构建产物的落点与 `frontendDist` 接线；窗口/focus/新标签页等 M1 未做的订阅型
方法（§5 表）；**首次真实 UI 渲染的 E2E**（开窗 → Host ready → 业务根挂载 → 至少一个真实业务
面可用）；以及把 M1 的「命令面验收台」保留为**开发用**页面而不是唯一前端（它是对拍与命令面回归
的工具，不能一删了之）。

---

### Task 4: 补齐 §5 登记的平台面缺口

**- [ ] Step 1–5: 大纲**
`getApplicationIcon` 的原生图标解码（Windows GDI；macOS `NSWorkspace` 半边**不在本阶段范围**：无 macOS 构建可验）、`saveFile` 的
`sourceUrl`（HTTP 取回 + 超时/大小上限）、其余订阅型方法、**深链单实例转发**（窗口模型：
Windows 上应用在跑时点 `mode://` 必须交给已有实例，不能再起新进程）。

---

### Task 5: **Windows 出包** + 体积门禁（把「可发布」做实）

**- [ ] Step 1–4: 大纲**
**Windows 出真实产物**；**实测两份预算的体积并分别对照门禁**（核心 ≤30MB；Node 运行时 +
sidecar externals 用 Task 2 的实测基准，见「体积门禁拆分登记」）；确认安装包内到底是什么
Node 运行时、放在哪、是否可被用户看见；产出交给 M5 的输入清单
（产物名、安装方式、更新链需要什么）。

**macOS / Linux 两条腿本阶段不做（2026-10-16 决定，登记而非省略）**：与该决定一致的是 M1 已经
把这两条 CI 腿挂成 `continue-on-error`（`docs/specs/tauri-platform-commands.md` §8.1），§8.3 也把
它们的风险登记为未解决。**解冻条件**：本阶段结束后重开，成本主要是 webkit2gtk/libsoup3 的
发行版差异（Linux）与 aarch64 目标确认（macOS），即 §8.3 那两条已知风险的收口；在此之前
**不要**把「三平台全绿」当作 M3 的完成条件。

---

## 阶段验收

1. Tauri 壳能渲染**真实产品 UI**（不是验收台），且 Host ready 判定在 30s 内成立；
2. §5 登记的四项平台面缺口关闭，深链不再起新进程；
3. **Windows 安装包全绿，且体积门禁按两份预算分别达标**（2026-10-16 Ruling 26 拆分，理由与算术见
   「体积门禁拆分登记」）：
   - **(a) 核心**（壳 + UI 资产 + Host bundle）——沿用原来的 **≤30MB**；Host bundle 已由 Task 2
     实测 **4.87 MiB**，壳与 UI 资产由 Task 5 实测；
   - **(b) Node 运行时 + sidecar externals**——**单列预算**，基准由 Task 2 实测：
     externals 传递闭包 **102.61 MiB** + `node.exe` **88.25 MiB**（单文件 gzip 后 33.91 MiB）；
     门禁数值由 Task 5 在该基准上定（压成安装包后的实际增量才是要管的数）。
   macOS / Linux **未验**，属**本阶段已知边界**（延后决定见「范围收窄登记」）；
   注意 `docs/plans/2026-10-09-tauri-rewrite-roadmap.md` 的 M3 验收原文写的是「三平台安装包体积
   保持核心 ≤30MB」——**本计划对路线图 M3 验收的这一条做了两处收窄**（平台数与门禁拆分），
   两处都是经人决定的，登记见下面两节；
4. `clippy -D warnings` 零告警、可跑用例全绿、`pnpm verify:pre-push` 通过；
5. 交给 M5 的输入清单就位（M5 据此切换发布线、配签名密钥与 `latest.json`）。

## 风险

| 风险 | 对冲 |
| --- | --- |
| **端口传输没有可行路线**（Task 0 全否） | 三种路线按「是否要改 renderer」排序逐一实测；走到需要改渲染层或新增 rpc 传输时**停下来向人确认**，不自行扩大范围 |
| 体积失控（Node 运行时 + host 闭包） | Task 2 已实测闭包与 bundle 体积（§「体积门禁拆分登记」）；**门禁本身拆成核心 / 运行时两份**，避免用一份不可达的数字当门禁（那样它只会长期变红）；DR-M3-2 已把「一份还是两份运行时」留给实测决定 |
| 原生模块 ABI（`node:sqlite`/`node-pty`/CUA）在 sidecar 里跑不起来 | **Task 2 已实测通过**（`node:sqlite` 真读写 + sidecar 内真起 pty，见 `docs/specs/tauri-renderer-host.md` §10.4/§12）：DR-M3-1 的前提成立 |
| 改渲染层踩 clean-room | renderer 是上游衍生文件：只读接口面；任何改动都写成 Task 0/Task 3 的产出与决策记录 |
| **三平台延后（macOS/Linux 出包）** | 对冲是「M5 前解冻」，且 Windows 上的体积 / Node 运行时结论对三平台**有可迁移性，但不构成验证**；M5 切换发布线需要三平台产物，因此这笔风险被推到 M5（见「范围收窄登记」） |
| 与 M2 线争抢同一 checkout / 同一批文件 | 两条线的文件面基本不重叠（M2 在 `crates/agent-*`，M3 在 `crates/sidecar-host` + `apps/desktop-tauri` + `packages/desktop/src/host-standalone`）；提交一律显式路径 |

---

## 范围收窄登记（2026-10-16，经人决定）

**收窄内容**：macOS / Linux 的**出包与体积验证**移出本阶段（Task 5 与阶段验收第 3 条已按此改写）；
`getApplicationIcon` 的 macOS 半边、以及 M5 的三平台演练一并顺延。

**动机与代价（必须一起读，不能只读「不做」三个字）**：

- 动机：这两条腿在本机无法构建/验证（本机只有 Windows + gnu 工具链；M1 §8.3 已把 Linux 的
  webkit2gtk/libsoup3 依赖与 macOS 的 aarch64 目标确认登记为**未解决风险**）。先上能验的那条腿，
  最快把「可发布」做实——与 DR-M3-1 / DR-M3-2 同一条判据（哪种快就先上哪种）。
- **代价：M5 切换发布线需要三平台产物**，所以这笔风险不是消失了，而是**被推到 M5**。
  Windows 上得到的体积与 Node 运行时结论对三平台**有可迁移性，但不构成验证**。
- **与上游权威的不一致（这条是登记的重点）**：`docs/plans/2026-10-09-tauri-rewrite-roadmap.md` 的
  M3 验收原文写的是「三平台安装包体积保持核心 ≤30MB」。本计划**收窄**了它，且这是**经人决定的范围
  收窄**；路线图文件本身不改（它是阶段权威，改写它等于把收窄藏起来），收窄只记在本计划与 M3 账本里。
- 解冻条件见 Task 5 里的那一段（§8.3 两条已知风险的收口）。

---

## 体积门禁拆分登记（2026-10-16 Ruling 26，经人决定）

**拆分内容**：产品阶段原来的单一「核心包 ≤30MB」门禁改成**两份分别测量、分别门禁的预算**：

| 预算 | 内容 | 基准（Task 2 实测）/ 门禁 |
| --- | --- | --- |
| **(a) 核心** | Tauri 壳 exe + UI 资产 + **Host bundle** | 已测：Host bundle **5,106,411 B（4.87 MiB）**；壳与 UI 资产由 Task 5 实测 → 沿用原门禁 **≤30MB** |
| **(b) 运行时 + 服务面** | **Node 运行时** + Host sidecar 的 external 依赖 | 已测：externals 传递闭包 **107,593,418 B（102.61 MiB）**；本机 `node.exe` **88.25 MiB**（单文件 gzip 33.91 MiB）→ 门禁数值由 Task 5 在该基准上定 |

**为什么必须拆（算术，不是感觉）**：

- 原门禁的定义面是 **NSIS 安装包字节**（`docs/specs/tauri-platform-commands.md` §8.2）。而单是
  `node.exe` 一个文件 gzip 后就有 **33.91 MiB**——**已经越过 30MB**；再加 sidecar 闭包，
  这个门禁在**选定架构下不可能达标**。
- 裁剪路线也救不回来：`node-pty` 的 `prebuilds/` **原始 57.82 MiB**，其中 `win32-x64` 29.65 MiB；
  裁掉非 Windows 与 `win32-arm64` 那两档**原始**省 28.17 MiB，但 `.node` 二进制的压缩比约
  **4.3:1**（复评给出的实测比），所以在**安装包**里只省下 **~6.6 MiB** 量级的字节。
  即「把 sidecar 裁干净」不足以把总包压回 30MB 以内。
- 门禁的**原始理由**已经不存在了：它是按「**不带 Node 核心** + sidecar 按需下载」写的
  （`docs/plans/2026-10-09-tauri-rewrite-roadmap.md`），而 **DR-M3-1 显式反转了这条**
  （为复用 `packages/services` 的原生资产而选择 Node sidecar，判据是「哪种快就先上哪种」）。
  门禁与架构互相矛盾，且此前从未被对齐——**继续留一条不可达的红线，等于把它变成长期假红**，
  而不是门禁。

**代价（必须一起读）**：

- **安装包会显著大于最初的目标**：以实测基准估算，Windows 安装包的 (b) 部分量级在
  **几十 MiB 到 ~100 MiB**（Node 运行时原始 88.25 MiB，压缩后只减一部分；sidecar 闭包 102.61 MiB
  里可裁剪的主要是 node-pty 的非目标预编译件与 `@types/node` 这类运行期不需要的包）。
  这是 DR-M3-1 的直接后果，不是执行失误。
- **(a)/(b) 的膨胀风险不同**：核心包仍受 ≤30MB 约束（Task 5 要实测并守住），
  (b) 走基准 + Task 5 定的门禁——所以「体积失控」这条风险仍有对冲，只是被放到了能管的那一半上。

**唯一能兑现原 ≤30MB 的替代方案**（写出来是为了让这个取舍对下一个读的人可见）：

- 把 Host 与服务层**用 Rust 重写**（不再复用 `packages/services`），或用系统自带运行时替代
  捆绑 Node——即 **DR-M3-1 拒绝的慢路**。那条路的代价是 M3–M4 的整体重做
  （45 个服务、原生资产、CUA 资产链都要重接）。**本计划不采用**，但它确实是唯一能在
  「不带运行时」前提下守住 30MB 的形态。

**与上游权威的不一致（登记的重点）**：`docs/plans/2026-10-09-tauri-rewrite-roadmap.md` 的 M3 验收
原文是「三平台安装包体积保持核心 ≤30MB」。本计划**重新定义**了它（平台数收窄 + 门禁拆分），
两处都是**经人决定**的；**路线图文件本身不改**（它是阶段权威，改写它等于把改动藏起来），
改动只记在本计划与 M3 账本里。

---

## 为什么 Task 1–5 只到步骤级

与 M2 计划同一条理由，这里更强：

1. **Task 0 的结论决定它们的形状。** 端口走哪条路线、渲染层要不要改，直接决定 Task 1 的传输实现、
   Task 3 的改动面，甚至 M4 的计划要不要跟着动。在 Task 0 之前写满代码块，产出的是**会作废的
   计划**——M2 计划用同一条理由把 Task 3–7 留到步骤级，事后证明是对的（M2 的 Task 2 就因实测
   推翻了计划的前提：只读切片实际只有 `Read`）。
2. **规模。** M3 按路线图是 3–5 个月，一次写成的「完整代码级计划」会大到没人能执行。

Task 0 之后各出子计划，落点：

| 任务 | 开工时应产出的子计划 |
| --- | --- |
| Task 1 最小 Host sidecar | `docs/plans/2026-XX-XX-m3-host-sidecar.md` |
| Task 2 真服务面 + startup 通道 | `docs/plans/2026-XX-XX-m3-services-startup.md` |
| Task 3 挂载存量 renderer | `docs/plans/2026-XX-XX-m3-renderer-mount.md` |
| Task 4 §5 平台面缺口 | `docs/plans/2026-XX-XX-m3-platform-gaps.md` |
| Task 5 Windows 出包与体积 | `docs/plans/2026-XX-XX-m3-packaging-size.md` |
