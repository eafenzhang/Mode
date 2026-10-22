# Rust 改造的「更轻更快」评估与重排（2026-10-23）

> 触发：Rust 改造的目标不只是体积变小，还要在**性能与效率**上提高。本计划先给出现状的
> 实测基线，再回答三个问题：**哪些可以删除**、**哪些模块需要重做**、**按什么顺序做**。
> 它**不推翻**既有决策（DR-01…DR-TUI-1 仍然有效），只把 M2/M4/M5 的口径从「写了但没有指标」
> 修订成「有指标、有基线、有阶段」。
>
> 前置阅读：`docs/plans/2026-10-09-tauri-rewrite-roadmap.md`（总图与 M0–M5）、
> `docs/plans/2026-10-16-tauri-agent-runtime.md`（M2 Rust 运行时，**代码面已完成**）、
> `docs/plans/2026-10-18-m4-overlap-reduction.md`（deferred 面）、
> `docs/specs/tauri-renderer-host.md` §13（体积与冷启动口径）。

## 0. 为什么现在要重排（三条事实）

1. **Rust 运行时已经建成，但产品路径没切**。M2 的 `crates/agent-core`（session loop /
   streaming / CommandInbox / 权限引擎）、`agent-tools`（Read/Write/Edit/Bash + diff 呈现）、
   `agent-protocol`（v4 帧镜像）已交付：30 suites / 202 passed，对拍矩阵 CI 门禁全绿
   （路线图 M2 节，2026-10-22 回填）。但安装形态仍由 Host sidecar **spawn Node 进程跑
   `mode.cjs`**（`bundledAgentRuntime.ts`）——Rust 运行时目前只是「验收资产」，不是「产品路径」。
2. **体积有预算，性能没有**。M5 只定义了「安装包字节」口径的核心预算（DR-M5-2b，≤30MB 待定），
   冷启动、流式延迟、常驻内存、常驻 CPU 均无门禁与基线。
3. **删除面已经算清但没有排期**。路线图登记的 deferred 重合面（services|shared|rpc|server|
   provider）是 514 个文件 / 121,881 行——这些正是「随 Rust 化一起消失」的模块，但没有一条
   路径把它们与「删 Node 运行时」绑定起来。

## 1. 现状基线（全部实测，2026-10-23 本机）

### 1.1 发运集体积（`dist-host-sidecar/ship/`，未压缩字节；门禁口径是安装包字节，见 §5）

| 组成 | 体积 | 说明 |
| --- | --- | --- |
| `node/` | **89 MB** | 随包 Node 运行时；**最大的单一项** |
| `host-sidecar/` | 35 MB | 打包后的 Host 单文件 bundle + external 依赖闭包 |
| `agent/glm/` | 20 MB | `mode.cjs` + 官方插件 + bundled-skills（与 Electron 包同一份产物） |
| `tools/` | 8.6 MB | cua-helper、browser-import、ripgrep |
| `config/` | 0.2 MB | 内置 provider 配置 |
| **合计** | **≈153 MB** | 未压缩载荷；压缩后另测 |

### 1.2 进程与 IPC 形态（产品路径）

```
壳(tauri, Rust)
 ├─ 常驻 sidecar（Node，host-standalone）── stdio 帧协议(JSON+base64) ── 壳侧转发
 │    └─ agent 运行时进程（Node，mode.cjs）── stdio 帧协议 ── host
 └─ CUA helper（独立进程，raw input socket）
```

- 常驻进程数：**3–4**（壳 / sidecar Node / agent Node / helper）。
- 帧协议：JSON + base64（`packages/rpc` 的 `SocketProtocol`/`VSBuffer`）；每帧一次 JSON 解析
  + 一次 base64 编解码 + 至少一次 `Buffer` 拷贝，是流式路径上的固定开销。

### 1.3 冷启动（已实测，`tauri-renderer-host.md` §13.5）

| 指标 | 中位数（release，N=7） | 备注 |
| --- | --- | --- |
| `T_ready`（Host ready） | **825 ms** | 含壳启动 + sidecar Node 启动 + bundle 解释 + 服务装配 |
| `T_surface`（首屏可交互） | **962 ms** | 再加 renderer 启动与挂载 |

debug 对照 1030 / 1359 ms。冷启动的大头是 **Node 进程启动与 bundle 解释**，不是 Rust。

### 1.4 代码量（`wc -l` 统计）

| 层 | TS 行数 | Rust 行数 |
| --- | --- | --- |
| `apps/mode-cli`（bootstrap 66k / adapters 53k / contracts 22k / dynamic-workflow 21k / core 17k / **tui 14k** / cli 11k …） | ≈208k | — |
| `packages/services`（mode-agent 20k / bots 20k / session 11k / …） | 109k | — |
| `packages/shared`（协议与校验） | 44k | — |
| `packages/ui`（React 渲染层） | 19k | — |
| `packages/desktop`（renderer/host/host-standalone） | 13k | — |
| `packages/server`（HTTP/WS + remote 9.8k） | 12k | — |
| `packages/rpc` | 3.5k | — |
| 16 个 crate（cua-ax 7.9k / agent-tools 3.8k / agent-core 2.1k / sidecar-host 2k / platform 1.3k / agent-protocol 1.1k …） | — | **≈23.5k** |

## 2. 目标：把「轻 / 快 / 省」写成指标

| 维度 | 指标 | 现状 | 目标 | 测量面 |
| --- | --- | --- | --- | --- |
| 轻 | 发运集「核心」载荷 | ≈63 MB（host-sidecar + agent + tools） | **≤ 25 MB** | 安装包字节（NSIS）+ 未压缩诊断 |
| 轻 | Node 运行时（核心发运集内） | 89 MB | **0**；npm-MCP 回退时按需出现（不再是随包常驻） | 发运集目录 |
| 快 | 冷启动 `T_ready` | 825 ms | **≤ 400 ms** | `measure:cold-start` 同口径 |
| 快 | 首屏 `T_surface` | 962 ms | **≤ 600 ms** | 同上 |
| 快 | 流式增量端到端 p95 | 未测 | **≤ 80 ms**（本机 loopback） | 新增 bench（§5） |
| 省 | 常驻进程数 | 3–4 | **2**（壳 + helper） | 进程快照 |
| 省 | 会话空闲内存 | 未测 | **≤ 150 MB**（壳含 renderer） | 新增 bench |

> 口径纪律沿用 DR-T5-1：每个数字必须标注测在哪张脸（安装包字节 / 未压缩载荷 / 进程内 RSS），
> 混用即视为无效。目标值是**承诺值**，第一轮 bench 落地后若与现状冲突，改目标而不是改尺子。

## 3. 可以删除的部分（三档）

### 3.1 立即可删（不依赖任何 Rust 重做，M4a）

| # | 删除项 | 规模 | 收益 | 风险与前提 |
| --- | --- | --- | --- | --- |
| 1 | 旧 Electron 线残留（main/preload/scheduler/electron-builder） | 326 文件已删 | 已完成（`d71ed992`） | — |

> **2026-10-23 用户裁决：SSH/WSL/Docker 后端保留**（含 `ssh2` 等依赖与
> `/ws/remote/:id` 桥）。原计划把 `packages/server/src/remote/` 的部署栈与后端列为
> 「立即可删」，判断有误：Tauri Host 的 SSH/WSL/Docker 远程工作区**直接依赖它**——
> `packages/desktop/src/host-standalone/remoteWorkspaceService.ts` 的
> `defaultConnectStdio` 动态 `import("@mode/server/remote")`，用
> `createRemoteBackend` / `connectRemote` / `pickRemoteRuntimeEnv` 建连（LR-4 第三阶段的
> SSH/WSL/Docker 路径）。删掉它们会直接断掉这条产品能力，而不是「少一个 CLI 遗留」。
> 因此该删除项撤销；体积收益改由 M4b（Host 并入壳）与 M5a（Node 运行时退场）承担。

### 3.2 随「Host 并入壳」消失（M4b）

| 删除项 | 规模 | 说明 |
| --- | --- | --- |
| `dist-host-sidecar/host-sidecar/`（bundle + external 闭包） | 35 MB | Host 服务面直接跑在壳的 async runtime 里，不再有独立进程与 bundle |
| `host-standalone/index.ts` 的 stdio 帧编解码与 `stdioFrames`/`stdoutGuard` | ≈2k 行 | 帧协议只在跨进程时需要；同进程直接函数调用 |
| `bundledAgentRuntime.ts`（`MODE_AGENT_SERVER_COMMAND` 注入） | ≈80 行 | agent 不再是独立 Node 进程 |

### 3.3 随「Agent 运行时切 Rust」消失（M5a，最大的一块）

| 删除项 | 规模 | 说明 |
| --- | --- | --- |
| 随包 Node 运行时 `node/` | **89 MB** | Tauri 形态不再需要 |
| `agent/glm/mode.cjs` + 官方插件 + bundled-skills | 20 MB | 被 `crates/agent-core` + `agent-tools` 取代 |
| `packages/services` 中的运行时面：`mode-agent` 20k / `session` 11k / `model-provider` 5.5k / `process` 2.3k / `skills` 1.4k / `subagents` 1.4k / `plugin-sync` 1.5k / `mcp-*` … | ≈43k 行 | 会话循环、流式、工具执行、模型调用、MCP host 的 TS 实现 |
| `apps/mode-cli` 的运行时装配：`bootstrap` 66k / `adapters` 53k / `core` 17k / `contracts` 22k（协议镜像）/ `cli` 11k / `dynamic-workflow` 21k + runtime 1.3k | ≈190k 行 | CLI 入口与 runtime；**TUI 14k 不在此列**（见 §3.4） |
| `packages/rpc` | 3.5k | 同进程内不再需要 ChannelServer/Client 两层代理 |
| `node-forge` / `ws` / `axios` / `undici` / `node-pty` / `yaml` / `smol-toml` / `yazl` | 8 个依赖 | 用 rustls/ring、tokio-tungstenite、reqwest、portable-pty、serde-yaml/toml、zip 替换 |

> 前提（**不可跳过**）：M2 的对拍 oracle 真源仍在 TS（DR-07）。删除 TS 运行时的前置条件是
> ① 对拍矩阵全绿保持、② 真实模型 20 个端到端任务验收通过（M2 阶段验收第 3 条）、
> ③ 新增的性能 bench 证明不回退。三者缺一不可，否则「对拍」会退化成「自己对自己」。

### 3.4 明确**不可删**（本阶段）

- `packages/ui` 19k + HTML/CSS（渲染层已是「轻」的形态，别动）。
- **TUI 14k**：DR-TUI-1 已定沿用 TS；只有当「CLI 要变成 Rust 单二进制」时才评估 ratatui。
- 插件 / 技能的 JS 执行面：**执行面保留**（插件仍是 JS），但承载方式按 2026-10-23 裁决改为
  「子进程 + 带嵌入 JS 引擎的自家二进制」，随包 Node 退为按需资产（见 §4 P2）。
- IM bot SDK（`@larksuiteoapi/node-sdk`、`@wecom/aibot-node-sdk`）：**2026-10-23 已裁决为可选
  feature**——不随核心桌面包安装、也不重写。落地口径：`packages/services/bots` 的 SDK 静态
  import 改为按通道动态 import，SDK 移出核心发运集闭包；未启用 bot 通道时加载失败不影响启动
  （实现项见 §6 M4a 可选项）。

## 4. 需要重做的模块（按收益/风险排序）

### P0-A：Host 并入壳 + IPC 换代（M4b，2–4 周）

- **做什么**：`host-standalone` 的服务装配（`createLocalServices` 的 Rust 等价物）搬进
  `mode-tauri` 的 async runtime（tokio + tauri state）；renderer↔Host 的通信从「stdio 帧 +
  JSON+base64」改为**同进程通道**（渲染层 RPC 仍走 WebSocket 或 tauri IPC，但 payload 换 CBOR）。
- **收益**：35 MB 载荷消失；一次 IPC 少两次 base64 + 一次 JSON 解析；`T_ready` 里 sidecar 启动段消失（保守估 200–400 ms）。
- **风险**：Host 服务面目前依赖 Node 生态（`node-pty`、`node-forge`、`better-sqlite3`）——
  这些必须先有 Rust 替代（portable-pty、ring、rusqlite），否则会陷入「为了并入壳先重写一半服务」。
  **建议前置任务**：先把 `settings / credentials / task-index sqlite / file ops` 四件最小面 Rust 化。

### P0-B：Agent 运行时切换产品路径（M5a，4–8 周，与 M2 验收衔接）

- **做什么**：Host 侧新增 `agent_runtime: rust | node` 双跑开关（默认 node，release 验收后翻转）；
  Rust 路径直接进程内调用（不经 stdio）；parity matrix 从「两侧夹具对拍」升级为「同一批夹具
  在两种运行时下跑」；翻转后删除 §3.3 全部内容。
- **收益**：Node 89 MB → 0；agent 20 MB → 0；常驻进程 4 → 2；流式事件少一层 JS 对象分配。
- **风险**：模型 provider、MCP host、动态工作流三块在 Rust 面还不完整（`agent-tools` 只有
  Read/Write/Edit/Bash）。**必须先补齐工具注册表与权限等级**，否则 parity 会红。

### P1-A：模型 provider / 凭据 / 设置 / 任务索引 Rust 化

- provider 5.5k TS + shared 44k 协议面 → Rust（reqwest + rustls 已在 `crates/platform` 用过）。
- `node-forge`（凭据加密）→ ring；`rusqlite` 替代 `node:sqlite`。
- 收益：P0 的前置；顺带把 shared 的 44k 协议面拆成「serde 真源 + TS 镜像」，减少双写。

### P1-B：CUA helper 合并入壳

- 当前 helper 是独立进程（`tools/cua-helper`），raw input socket 通信。合并进壳可再省一个进程与一份 IPC。
- 风险：权限边界（helper 独立是**有意**的安全隔离，见 `lan-remote-computer-use` spec）；
  **不建议在没有安全评审前合并**，先量化它的内存与启动成本再决定。

### P2：插件宿主形态（**2026-10-23 已裁决：A + 按需回退**）/ IM bot Rust 化 / TUI ratatui

**插件宿主（方向已定）**：**A——维持 bundle-only 契约，插件仍跑在独立子进程（DR-PH-1），
但子进程的可执行文件从「随包 Node」换成「带嵌入 JS 引擎（QuickJS 级）的自家二进制」**。

- 协议不变（DR-PH-2 的 NDJSON JSON-RPC over stdio），资产层（`crates/plugin-host` 的
  manifest/AssetCache）与 CDN 按需下载不变；
- **稳定性**：子进程崩溃隔离保留（不做进程内嵌入，避免插件 OOM/死循环拖死宿主）；
  无 native ABI/DLL 依赖；CDN 上的 bundle 是冻结字节，没有 `postinstall` 与版本漂移；
- **通用性**：我们的插件本来就是单文件 bundle，作者契约零变化；native addon 类插件
  明确不支持（fail closed，错误可诊断）；
- **MCP 的通用性由按需回退承担**：MCP 是 stdio 子进程协议，宿主只需能 spawn 可执行文件；
  npm 分发的 MCP server 没有二进制时，按需下载 JS 运行时资产（sha256 + CDN，复用
  AssetCache 骨架）——89 MB 的 Node 从「随包常驻」变成「装了 npm MCP 才出现」。

**排序约束（硬）**：M5a 的「`node/` 从**核心**发运集消失」以本项（或其回退链路）就绪为
前置；否则删掉 Node 会让所有插件与 npm MCP 直接失效。

其余两项（IM bot Rust 化、TUI ratatui）仍是按需评估，不进本次承诺。

## 5. 指标与测量（M4a 先行）

新增 `apps/desktop-tauri/bench/` 与脚本（`pnpm measure:*`），**在改任何代码之前先把基线量出来**：

1. `bench/cold-start`：现有 `measure:cold-start` 扩展为 T_ready / T_surface / T_agent_ready 三段。
2. `bench/stream-latency`：本机 loopback，从 `text_delta` 事件产生到 renderer 收到的时间（p50/p95）。
3. `bench/memory`：常驻 30 分钟的 RSS 曲线（壳 / sidecar / agent 三个进程分别采样）。
4. `bench/ipc-throughput`：帧吞吐（MB/s）与每帧分配次数（`--prof` 或 `heap snapshot`）。

验收门禁沿用 DR-T5-1：**每个数字标注测量面**；基线入库后，任何性能改动都必须在同一面上给出前后对比。

## 6. 分阶段

| 阶段 | 内容 | 交付判据 | 依赖 |
| --- | --- | --- | --- |
| **M4a（1–2 周）** | bench 脚本与基线入库（§5 四项）；发布门禁补一条「发运集核心载荷 ≤ 25MB」的**观测**项。**本阶段不做删除**（SSH/WSL/Docker 后端已裁决保留，其余删除全部依赖 M4b/M5a）。**可选项**：IM bot SDK 可选化（larksuite/wecom SDK 改按通道动态 import，移出核心发运集闭包） | bench 全绿并写入 roadmap 度量表；三类指标的基线数字落库；若做可选项：未启用 bot 通道时启动不受影响，SDK 不出现在核心闭包清单 | 无 |
| **M4b（2–4 周）** | settings/credentials/task-index/file-ops 最小面 Rust 化 → Host 并入壳 → IPC 换代（CBOR）；`T_ready` ≤ 500ms | §1.3 表同口径复测；35 MB 载荷从发运集消失 | M4a |
| **M5a（4–8 周）** | Agent 工具注册表补全 → 双跑开关 → parity 升级 → 翻转默认值 → 删除 §3.3 全部内容；**前置：插件宿主 A 形态（或 npm-MCP 按需 JS 运行时回退）就绪** | 对拍矩阵全绿；真实模型 20 任务验收；`T_ready` ≤ 400ms、`node/` 从**核心**发运集消失；插件与 npm MCP 在无随包 Node 下可用 | M2 验收 + M4b + 插件宿主 A |
| **M5b（收口）** | provider/凭据/共享协议真源翻转；第三方台账 514 缺失项复核；三指标仪表转成 CI 硬门禁 | 三门禁（体积/语言/重合度）+ 新增性能门禁全绿 | M5a |

## 7. 风险与开放决策

1. **oracle 真源**：删除 TS 运行时前，它仍是行为真源（DR-07）。「双跑」与「删除」之间必须有明确的翻转点，写进 `agent-runtime-parity-matrix.md`。
2. **插件生态（已按 A 裁决）**：边界变为「native addon 插件不支持」与「按需 JS 运行时资产的下载/校验链路可用性」；AssetCache 骨架已有，风险从「放弃生态」转为「回退资产的体积与可用性」。
3. **macOS/Linux**：本计划所有测量面目前只有 Windows（Ruling 20）；跨平台真机验收仍是 M5 的独立风险。
4. **Clean-room 纪律**：重写任何模块时，旧实现体只能黑盒对拍，不得读上游衍生实现（`clean-room-policy.md`）。
5. **产品确认项：已全部裁决**（2026-10-23）——SSH/WSL/Docker 后端保留；IM bot SDK 可选 feature；插件宿主 = A（bundle-only + 子进程 + 自家二进制）+ npm-MCP 按需 JS 运行时回退。**当前无待拍板项**。

## 8. 与既有计划的关系

- **路线图 M4/M5**：本计划把「重合度清零」「第三方台账」「发布工程」之外的第四根轴——**性能**——补上，并把它挂进 M4/M5 的验收门禁。
- **`2026-10-16-tauri-agent-runtime.md`**：M2 的实现细节与对拍契约不变；本计划只负责「什么时候可以删掉 TS oracle」。
- **`2026-10-18-m4-overlap-reduction.md`**：deferred 面（514 文件）与本计划 §3.3 是同一张删除表，本计划为它提供了删除顺序与前置条件。
- **`2026-10-16-m3-packaging-size.md`**：两份体积预算继续有效；本计划新增「核心载荷 ≤ 25MB」为观测目标。
