# M3 生态域 1：插件 / MCP / Skills 的 Node sidecar 宿主 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: `subagent-driven-development`（推荐）或
> `executing-plans` 逐任务实现。步骤用 `- [ ]` 复选框跟踪。
>
> 本文件是路线图 **M3 生态域**的第一份子计划（插件宿主）。M3 的「Tauri 可发布」子计划
> （`docs/plans/2026-10-16-tauri-m3-ecosystem.md`）已经落地；本文件处理它**没有覆盖**的
> 插件/MCP/Skills 生态。IM bot / LAN / 远程 / Web / TUI 各自另出子计划。
>
> **开工前必读**：路线图 DR-01（插件宿主 = Node sidecar，按需下载）、
> M3 计划 DR-M3-1/DR-M3-2（Host sidecar 随包发布；插件负载按需下载）、
> `docs/specs/plugin-store-availability.md`（插件数据与状态所有者）、
> `docs/specs/clean-room-policy.md`。

**Goal:** 让插件 / MCP / Skills 在 **Node sidecar** 里运行，并把它们的工具注入 Rust 运行时；
插件负载按 DR-01 按需下载，核心离线可用；存量 `installed_plugins.json` / `.mode-plugin`
数据格式原样兼容。

---

## 现状与边界（先读这段，避免重复 Host sidecar 的工作）

| 事实 | 出处 | 影响 |
| --- | --- | --- |
| Host sidecar 已随包发布，负责 `IServiceAccessor` 与数据库启动通道 | `docs/specs/tauri-renderer-host.md`、M3 Task 1/2 | **不是**插件宿主；插件工具要注入的是 Rust `agent-core`，不是渲染层 |
| DR-01：插件宿主用 Node sidecar，首启按需下载、校验 sha256、健康探测 | 路线图 | 本计划的骨架 |
| DR-M3-2：Node 运行时随 Host 发布；插件/MCP/Skills 的**负载**按需下载 | M3 计划 | 复用已发布的 Node 运行时，不重复打包 Node |
| 插件安装记录：`installed_plugins.json`（marketplace root）与内置官方 cache | `packages/shared/src/plugin-types.ts`、`packages/services/src/plugins/installedPluginRoots.ts` | M3 **只读**，不做迁移 |
| Rust 工具注册表目前是静态的（Read/Write/Edit/Bash/TaskOutput/TaskStop） | `crates/agent-tools/src/lib.rs` | 需要**注入接口**，不能把插件工具硬编码进注册表 |
| MCP 子进程启动约 5s | spec `tauri-agent-runtime.md` §6.3 | 健康探测与工具列表要有超时/缓存，不能阻塞每个 turn |

**不在本计划：** 插件市场 UI / 安装流程（归渲染层与 Host services）；IM bot、LAN、远程、
Web、TUI；把插件用 Rust 重写；插件代码的版权归属（第三方，按 DR-01 不纳入自研口径）。

---

## 决策记录（开工前先读）

### DR-PH-1：独立插件 sidecar，复用已发布的 Node 运行时

- 插件宿主是**独立 child process**（不是 Host sidecar 进程内的一段代码）：崩溃隔离、可单独重启，
  且不会因为插件把 Host 的服务面一起拖死。
- 可执行文件用**已随包发布的 Node 运行时**（DR-M3-2 的那一份），不新增第二份 Node。
- 与 Host sidecar 的关系：两者都连同一个数据根，但**不共享进程**；插件 sidecar 由 Rust 运行时
  spawn/监督，Host 由 Tauri 壳 spawn/监督。

### DR-PH-2：协议用 NDJSON JSON-RPC over stdio

- 方法：`host/hello`、`plugin/listTools`、`plugin/callTool`、`plugin/health`、`host/shutdown`。
- 工具名沿用存量约定：MCP 工具是 `mcp__<server>__<tool>`；Skill 与插件工具用其既有名字。
- `plugin/listTools` 返回 name / description / inputSchema / family（若存量声明有），
  Rust 侧把它们注入 `agent-tools` 的**会话级注册表**（不改静态 `registry()`）。
- 错误帧与 CLI 的 `tool_execution_failed` 同形；未知工具仍是 `tool_not_found`。

### DR-PH-3：负载按需下载（manifest + sha256 + 缓存目录）

- 资产清单由 CDN 提供（复用 `MODE_REMOTE_ASSET_CDN_BASE_URL` / `prepare:remote-assets` 基建）；
  缓存目录在数据根下（建议 `<dataRoot>/plugin-host/<version>/`）。
- 首启：有缓存 → 校验后直接用；无缓存 → 下载 → 校验 sha256 → 解包；**断网且无缓存** →
  插件工具不可用，`plugin/listTools` 返回可读的 `plugin_host_unavailable`，核心功能不受影响。
- 卸载/升级：按 version 目录隔离，失败保留旧版本，不半删。

### DR-PH-4：插件工具走同一套权限引擎

- 插件工具必须经过 `permission::evaluate`（Task 6）；风险分级**先观察旧运行时**再定，
  不按直觉给 allow/ask。
- `allow_project` 的规则按工具名 + ruleContent 复用现有 `Ruleset`（D1 已裁决 Rust 直连 sqlite）。

### DR-PH-5：数据格式不迁移

- 只读 `installed_plugins.json`、`.mode-plugin` 清单、内置官方 cache；
- 写路径（安装/卸载）仍归 Host services / 渲染层，本计划不新增第二条写路径。

---

## Task 0：观察面（先做，阻塞后续）

**Files:**
- Create: `packages/desktop/tests/agent-runtime-oracle/probe-plugin-host.mts`
- Create/Modify: `docs/specs/plugin-host.md`（新 spec，记录观察）

- [x] **Step 0.1：盘点插件数据面**（2026-10-18，部分）：`.mode-plugin/plugin.json` 的字段、
      `installed_plugins.json` 的记录类型、`.agents/plugins/marketplace.json` 约定已入 spec §1；
      **隔离数据根下的确切文件路径尚未取证**（列为未观察）。
- [ ] **Step 0.2：观察旧运行时的插件工具面**（2026-10-18，部分）：默认隔离环境的 21 条里
      已抓到 `mcp__node_repl__js` / `Skill` / `Agent` 的完整声明；**fixture 插件安装后的清单
      变化尚未观察**。
- [ ] **Step 0.3：观察工具调用与生命周期**（2026-10-18，部分）：`Skill` 缺参/未知名字与
      未知 MCP 名字的错误形状已录；**MCP 启动耗时、负载缺失/断网/崩溃形状未观察**。
- [ ] **Step 0.4：观察权限面**（2026-10-18，部分）：`Skill` 与未知 MCP 都没有审批请求；
      **有副作用的插件工具（如 `mcp__node_repl__js`）是否审批未观察**，DR-PH-4 暂不实现。
- [x] **Step 0.5：把结果写进 `docs/specs/plugin-host.md`**（2026-10-18），未观察项已逐条列出。

**Step 0 的产物**：新 spec + 可复现探针；没有观察到的形状不写进实现。

---

## Task 1：资产清单与按需下载

**Files:**
- Create: `crates/plugin-host/`（Rust crate：下载/校验/缓存/进程监督）
- Create: `tools/plugin-host/`（sidecar 入口与 bundle；语言政策：TS）
- Test: `crates/plugin-host/tests/asset_cache.rs`

- [x] **Step 1：定义 manifest 与缓存布局**（2026-10-18，spec §5.1）：`schemaVersion` / `version` /
      `platform` / `entry` / `assetUrl` / `sha256`；单文件 bundle；缓存
      `<dataRoot>/plugin-host/<version>/<entry>`。
- [x] **Step 2：实现下载/校验与失败语义**（2026-10-18）：`crates/plugin-host` 的
      `PluginHostManifest` + `AssetCache`；缓存优先、sha256、临时文件 + rename、不半写。
      **Deviation**：资产按单文件 bundle 处理（Node 运行时随 Host 发布），本任务不做解包。
- [x] **Step 3：测试**（2026-10-18）：本地 HTTP 服务 5 条——下载、缓存命中不联网、sha 篡改、
      断网 + 有效缓存、entry 路径逃逸。
- [x] **Step 4：提交。**

---

## Task 2：sidecar 骨架、协议与健康

**Files:**
- Create: `tools/plugin-host/src/main.ts`（NDJSON stdio）
- Create: `crates/plugin-host/src/protocol.rs`（帧类型与严格校验）
- Test: `crates/plugin-host/tests/protocol.rs`

- [ ] **Step 1：协议类型 + 往返测试**（`host/hello`、`plugin/listTools`、`plugin/callTool`、
      `plugin/health`、`host/shutdown`）。
- [ ] **Step 2：sidecar 入口**：启动后发 hello，按请求返回；未知方法返回错误帧。
- [ ] **Step 3：Rust 侧 spawn/监督**：超时、崩溃重启（退避）、退出时不留孤儿。
- [ ] **Step 4：提交。**

---

## Task 3：插件发现与工具列表注入

**Files:**
- Modify: `crates/agent-tools/src/lib.rs`（会话级工具注册表注入接口）
- Modify: `crates/agent-core/src/session.rs`（turn 开始时拉取插件工具）
- Test: `crates/agent-core/tests/plugin_tools.rs`

- [ ] **Step 1：会话级注册表**：静态 `registry()` 保持不变；新增 `Session` 持有的
      `extra_tools: Vec<ToolSpec>`，查找/执行时先查静态表再查插件表。
- [ ] **Step 2：`plugin/listTools` → `ToolSpec` 投影**（schema 逐字保留）。
- [ ] **Step 3：测试**：fixture sidecar 返回两个工具；`lookup` 能命中，未知名字仍是
      `tool_not_found`。
- [ ] **Step 4：提交。**

---

## Task 4：`plugin/callTool` 桥接与权限

- [ ] **Step 1：调用桥**：Rust 工具调用 → `plugin/callTool` → 结果映射为 `ToolOutcome`；
      错误映射沿用 `tool_execution_failed` / 插件自定义错误。
- [ ] **Step 2：权限**：按 Step 0.4 的观察把插件工具接进 `permission::evaluate`；
      `allow_project` 复用 `Ruleset` 与 sqlite 持久化。
- [ ] **Step 3：测试**：成功、失败、超时、sidecar 崩溃中途。
- [ ] **Step 4：提交。**

---

## Task 5：MCP 与 Skills

- [ ] **Step 1：MCP 服务器发现**（配置来源以 Step 0.1 的表为准）；工具名前缀
      `mcp__<server>__<tool>`。
- [ ] **Step 2：Skills 列表与调用**（若存量 Skills 只暴露提示词而非工具，按观察结果建模，
      不硬塞成工具）。
- [ ] **Step 3：启动耗时与缓存**：MCP 约 5s，会话开始时并发拉起，不阻塞纯文本 turn。
- [ ] **Step 4：测试 + 提交。**

---

## Task 6：离线、重启与端到端

- [ ] **Step 1：断网 + 无缓存**：核心功能全在，插件工具返回 `plugin_host_unavailable`。
- [ ] **Step 2：sidecar 崩溃重启**：退避后恢复；不产生孤儿进程。
- [ ] **Step 3：E2E**：装一个 fixture 插件 → 工具出现在清单 → 调用成功 → 卸载后消失。
- [ ] **Step 4：`cargo clippy -D warnings` + 相关测试全绿；提交。**

---

## 验收

1. 已安装插件/MCP 的工具出现在 Rust 运行时并可执行，未知工具仍是 `tool_not_found`；
2. 断网且无缓存时核心功能不受影响，插件工具给出可读错误；
3. sha256 不符/半包被拒绝，缓存不半删；
4. sidecar 崩溃可重启，退出不留孤儿；
5. `installed_plugins.json` / `.mode-plugin` 数据格式未被改写（写路径仍唯一）。

## 风险

| 风险 | 对冲 |
| --- | --- |
| 插件工具面/Permission 形状未观察 | Task 0 先做；观察不到的不实现 |
| MCP 启动 5s 拖慢首个 turn | 会话开始时并发拉起 + 缓存；纯文本 turn 不等 |
| 与 Host sidecar 抢数据根/DB | 两进程只读同一数据根；写路径仍归 Host（D1 的 sqlite 是唯一例外） |
| 按需下载引入首启失败 | 缓存优先；失败降级为「插件不可用」而不是失败屏 |
| 插件工具绕过权限 | 所有插件工具统一走 `permission::evaluate`，未知风险按观察值处理 |
