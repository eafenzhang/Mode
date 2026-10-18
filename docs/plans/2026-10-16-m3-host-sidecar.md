# M3 Task 1：最小 Host sidecar Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `subagent-driven-development`（推荐）或
> `executing-plans` 逐任务实现。步骤用 `- [ ]` 复选框跟踪。
>
> **开工前必读两份**：阶段计划 `docs/plans/2026-10-16-tauri-m3-ecosystem.md`（决策记录 DR-M3-1/2/3
> 与 Global Constraints），以及 spike 结论 `docs/plans/2026-10-16-m3-port-transport-spike.md`
> 的 **§3/§4**——本任务的传输实现完全照它选定的路线走，**不要重新发明**。
> 〔注：spike 文档正在 fix round 1 中收尾（补可复现的复现面、把投递时序约束移进 §4），
> 开工时以最新版为准；若 §4 的约束与本文冲突，以 spike 文档为准并回报。〕

**Goal:** 让 Tauri 壳能 spawn 一个 Node sidecar 承载 Host，并把一个满足 `MessagePortLike` 的端口
（承载 `ChannelServer`）与 database-startup 状态流送进 webview——使渲染层的启动闸门放行，
业务根能挂载。**本任务只到「壳 → Host → 端口 → 渲染层」这条链打通**，服务实现仍为空壳。

**Architecture:** 三步流水线。**Rust 壳**建窗口（必须由 Rust 建，才能在页面加载前挂
`initialization_script`），起 sidecar 并监督它，注册接收 raw 帧的 command；**页面侧脚本**在页面内
创建 `MessageChannel`，把 `port1` 交给渲染层、`port2` 桥回壳；**Node sidecar** 把壳转发来的 stdio
帧喂给 `packages/rpc` 的 `ChannelServer`，Server 背后是一个**全是 stub** 的 `IServiceAccessor`。

**Tech Stack:** Rust 1.99 + tauri 2.12（`initialization_script`、自定义 command、raw 响应）、
Node 24（sidecar 运行时）、`packages/rpc`（`ChannelServer`）、`packages/shared`
（`InternalChannels` / `DatabaseStartupState`）。

**Spec:** 本任务的实现 spec 于本任务内产出：`docs/specs/tauri-renderer-host.md`（Host 契约：
端口传输、startup 状态流、失败语义、以及「哪些部分由自动化测试覆盖、哪些必须人工跑」）。
纪律：`docs/specs/clean-room-policy.md`——`crates/**` 与 `apps/desktop-tauri/**` 全部适用；
**函数体范围不是接口面**（2026-10-16 的 spike 在这一点上超出过许可，记为偏差，别再犯）。

## Global Constraints

- **不改渲染层**：spike 的结论是渲染层**不需要改**，这正是选这条路线的理由。若实现中发现必须改
  `packages/desktop/src/renderer/**`，**停下来回报**——那会改变 M4 的计划，属范围变更。
- **端口必须在页面内创建**：tauri 2.12.4 / wry 0.57.0 **没有** port/transferable 能力，Rust 侧
  无法转移端口。不要试图在 Rust 里造端口再递给页面。
- **页面必须是本地 asset-protocol 内容**：远端 `http://` origin 永远调不了 app command。
- **raw 响应是 `ArrayBuffer`**：喂给端口前必须包成 `Uint8Array`，否则 `MessagePortProtocol` 的
  `instanceof Uint8Array` 检查会**静默丢帧**。
- **投递时序是语义**：端口与 startup 状态必须在渲染层的监听器已存在之后才投递
  （spike 实测：延迟到 `DOMContentLoaded` / `readyState === "interactive"`）。
- **数据隔离**：测试与本地跑 **绝不** 碰用户真实 `~/.mode`（沿用既有口径：
  `MODE_DATA_BASE_DIR` + `HOME`/`USERPROFILE` 双覆写，并对隔离生效做断言）。
- **语言政策**：新增 Rust / TS / HTML / CSS；新脚本 `.ts`、新测试 `.mts`。
- **每个任务结束**：`node scripts/rust-cargo.ts clippy --workspace --all-targets -- -D warnings`
  零告警、可跑用例全绿、`pnpm verify:pre-push` 通过。
- **macOS / Linux 不在本阶段范围**（Ruling 20）：只做 Windows，别引入只在别的平台成立的分支。

---

## 决策记录（开工前先读）

### DR-T1-1：本任务的「成功」定义是 stub 也能放行

渲染层的闸门是：收到 `DatabaseStartupState.phase === "ready"` **且** 端口的 `databaseStartupId`
与 `state.startupId` 匹配，才会挂业务根。所以 Task 1 的目标**不是**服务可用，而是**闸门放行**。
stub accessor 的每个方法都返回明确的 `not_implemented`（不许返回 `undefined` 或空对象——
那会让调用方把「没实现」读成「成功但没数据」）。

依据：这与 M1 的做法一致（命令面 shim 对未实现的方法显式抛
`not_implemented_until_m3`，而不是静默返回假值）。**代价**：Task 1 结束时 UI 会挂载但在几乎所有
交互上失败——这是**预期的中间态**，Task 2 才接真服务。不要把「UI 能显示」当成「能发布」。

### DR-T1-2：端口往返的自动化边界（诚实划界，别假装 CI 能验）

端口路径的**后半段**需要真实 webview；本仓 M1 已明确不引入 `tauri::test` 的 webview mock 运行时。
因此分两层：

| 层 | 覆盖方式 | 能否进 CI |
| --- | --- | --- |
| 页面侧转移逻辑（端口拿得到、`Uint8Array` 包裹、投递时序、帧往返） | Node 里用**真实** `MessageChannel` 起一个页面侧 harness，喂给真 `ChannelServer`，断言一次 `Initialize` + 一次 RPC 往返 | **能**（不需要 webview） |
| 壳 ↔ webview 的注入与 raw 帧泵 | **opt-in 集成测试**（带 flag 才建真窗口并断言往返），默认跳过 | 不能，且**必须在每次发版前人工跑** |

**代价（必须写进 spec 与 M5 的输入清单）**：CI 无法完整验证端口路径，所以「发版前人工跑一次
opt-in 集成测试」是一条**发布流程上的硬要求**，不是可选项。漏掉它等于「端口在某次改动后静默失效、
而发布流程说全绿」——这正是这个仓库反复吃亏的那类失败。

### DR-T1-3：sidecar 的生命周期归 Rust 壳，且必须能不留孤儿

壳负责 spawn 与监督（崩溃重启策略、退出时清理）。**判据**：壳正常退出后，进程表里不得残留
sidecar；sidecar 崩溃时壳要能观察到并把 `phase: "failed"` + `errorCode` 推给渲染层，而不是
让渲染层等到 30s 超时（超时是兜底，不是主路径）。

---

### Task 1: 最小 Host sidecar（骨架 + 端口通道 + 闸门放行）

**Files:**
- Create: `crates/sidecar-host/{Cargo.toml,src/lib.rs,src/spawn.rs,src/transport.rs}`
- Create: `packages/desktop/src/host-standalone/{index.ts,stubAccessor.ts}`
- Create: `apps/desktop-tauri/test/portRoundTrip.test.mts`（页面侧 harness，进 CI）
- Create: `apps/desktop-tauri/test/webviewRoundTrip.test.mts`（opt-in，默认 skip）
- Modify: `apps/desktop-tauri/src-tauri/src/main.rs`（改用 Rust 建窗口 + 挂 init script + 注册 command）
- Modify: `apps/desktop-tauri/src-tauri/src/lib.rs`（若需要）
- Create: `docs/specs/tauri-renderer-host.md`

**Interfaces:**
- Produces（Rust）：`sidecar_host::spawn(app: &AppHandle) -> Result<HostHandle>`；
  `HostHandle::shutdown(self) -> Result<()>`；以及壳侧两个 command：接收页面 raw 帧、返回 raw 帧
  （具体名以 spike 文档 §3 为准，**不要另起名字**）。
- Produces（Node）：`packages/desktop/src/host-standalone/index.ts` 的可执行入口，读 env 决定
  data root，起 `ChannelServer` 并暴露 stub `IServiceAccessor`。
- Produces（页面侧）：满足 `MessagePortLike` 的对象递到渲染层，配合 `DatabaseStartupState` 流使
  闸门放行。

- [x] **Step 1: 把端口往返立成提交的测试（本任务的第一个交付，RED 先行）**
  〔2026-10-19 核对：`apps/desktop-tauri/test/portRoundTrip.test.mts` 已在 `pnpm --filter @mode/desktop-tauri test` 中通过（同批 35 pass / 0 fail / 7 opt-in skip）。〕

按 DR-T1-2 的第一层写 `apps/desktop-tauri/test/portRoundTrip.test.mts`：脚本里建**真实**
`MessageChannel`，把 `port2` 接给一个真的 `ChannelServer`（背后是 stub accessor），`port1` 经
「`ArrayBuffer` → `Uint8Array` 包裹 → 帧往返」这条与生产一致的路径，断言一次 `Initialize`
与一次 RPC 往返。
Run: `pnpm --filter @mode/desktop-tauri test`
Expected: **FAIL**——`ChannelServer` 尚不存在（这是 RED；把失败输出记下来）。

- [x] **Step 2: 实现 Node sidecar 入口与 stub accessor**
  〔2026-10-19 核对：`packages/desktop/src/host-standalone/{index,serviceAccessor}.ts` 与 `packages/desktop/tests/host-standalone.test.mts` 已落地。〕

`packages/desktop/src/host-standalone/`：入口读 env 决定 data root、起 `ChannelServer`
（`packages/rpc` 的公共入口，**不要 deep import**）、背后是 stub accessor（每个方法显式
`not_implemented`，见 DR-T1-1）。
Run: 同 Step 1。
Expected: PASS。

- [x] **Step 3: Rust 壳 spawn 与监督 sidecar**
  〔2026-10-19 核对：`crates/sidecar-host` 已存在（含 `host_lifecycle`/`host_stderr` 用例）；本机 `cargo test` 受 MSVC `link.exe` 环境阻塞，未在此环境复跑。〕

`crates/sidecar-host`：`spawn` 起 Node 入口（路径由 env 或资源目录解析，**不硬编码绝对路径**）、
把 stdio 帧接到传输层、监督子进程。按 DR-T1-3 写**退出清理**与**崩溃上报**两条用例。
Expected: 用例绿；壳正常退出后无残留进程。

- [x] **Step 4: 壳改用 Rust 建窗口 + 挂 init script + 注册 raw 帧 command**
  〔2026-10-19 核对：`main.rs` 用 `sidecar_host::bridge::bridge_script` + `WebviewUrl::App(pick_shell_entry(...))` 建窗；`windowModeInjection.test.mts` / `productUiMount.test.mts` 已在仓。〕

按 spike 文档 §3 的实现骨架做：窗口在 Rust 建（`WindowConfig` 没有 init script 字段），
页面侧脚本**在页面内**创建 `MessageChannel`，`port1` 交给渲染层、`port2` 桥回壳；
**投递延迟到 `readyState === "interactive"`**（Global Constraints 里的时序语义）。
Command 的 ACL 需求以 spike 实测为准。
Expected: 起壳后，前端页面里能看到注入的端口对象（用验收台页面打印）。

- [ ] **Step 5: opt-in 集成测试 + 闸门放行验收**
  〔2026-10-19 核对：`webviewRoundTrip.test.mts` 已建（默认 skip）；真窗口 30s 放行仍是人工项。〕

写 `webviewRoundTrip.test.mts`（默认 skip，带 flag 才建真窗口）；然后人工跑一次：
壳起来 → 渲染层在 **30s 内**看到 `phase: "ready"` 且 id 匹配 → 业务根挂载（此时会因 stub
而大量失败，**这是预期的**，见 DR-T1-1）。
Expected: 记录实际输出；把「必须人工跑」这条写进 `docs/specs/tauri-renderer-host.md` 与 M5 输入清单。

- [x] **Step 6: 提交**
  〔2026-10-19 核对：提交已存在（`057b7e9d` 起）。〕

```bash
git add crates/sidecar-host packages/desktop/src/host-standalone apps/desktop-tauri docs/specs/tauri-renderer-host.md Cargo.toml Cargo.lock
git commit -m "feat(tauri): M3 Task 1——最小 Host sidecar 与端口通道，闸门放行"
```

---

## 阶段验收

1. 壳能 spawn/supervise Node sidecar，正常退出不留孤儿，崩溃能上报 `phase: "failed"`；
2. 页面侧端口往返测试进 CI 并绿（DR-T1-2 第一层）；
3. opt-in 集成测试可跑，且**人工实测过**「渲染层 30s 内放行」；
4. **渲染层未被修改**（`git diff` 对 `packages/desktop/src/renderer/**` 为空）；
5. `clippy -D warnings` 零告警、可跑用例全绿、`pnpm verify:pre-push` 通过；
6. `docs/specs/tauri-renderer-host.md` 就位，含「哪些由自动化覆盖、哪些必须人工跑」的明确划界。

## 风险

| 风险 | 对冲 |
| --- | --- |
| 端口路径只在真 webview 下成立，CI 覆盖不到 | DR-T1-2 的两层划界 + 「发版前人工跑」写进 M5 输入清单；漏跑等于静默失效 |
| stub accessor 被调用方读成「成功但没数据」 | DR-T1-1：每个方法显式 `not_implemented`，不返回 `undefined`/空对象 |
| 实现中被迫改渲染层 | **停下来回报**（那会改 M4 计划），不在本任务里自行扩大范围 |
| sidecar 的 Node/原生依赖在壳环境下不可用 | 本任务不需要真服务，先把链打通；原生依赖（`node:sqlite`/`node-pty`）在 Task 2 第一件事就验 |
