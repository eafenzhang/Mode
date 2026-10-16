# M3 Task 3：把存量 renderer 挂上 Tauri 壳 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `subagent-driven-development`（推荐）或
> `executing-plans` 逐任务实现。步骤用 `- [ ]` 复选框跟踪。
>
> **开工前必读**：阶段计划 `docs/plans/2026-10-16-tauri-m3-ecosystem.md`（Global Constraints、
> DR-M3-1/2、体积门禁拆分登记）、Host 契约 `docs/specs/tauri-renderer-host.md`（§已知危险、
> §15 的登记项）、M1 的平台面 spec `docs/specs/tauri-platform-commands.md`（§1 命令切片表、
> §4.2 的「renderer 为什么挂不上」证据表、§5 未完成项）。

**Goal:** 让 Tauri 壳渲染**真实产品 UI**（不是命令面验收台），并在其中至少跑通一个真实业务面——
把「Host 已能提供真服务」变成「用户真的在用产品」。

**Architecture:** Task 1/2 已经把「壳 → Host → 端口 → 渲染层」这条链打通，且 Host 提供真服务。
本任务做两件事：**把前端从验收台换成真实 renderer 的构建产物**，以及**补上 renderer 启动时
需要而 Tauri 没有的那一层注入**（`window.mode`）。

**Tech Stack:** 同前（Rust 1.99 + tauri 2.12 + Node 24），外加 `packages/ui` 的适配层与
`packages/desktop` 的 renderer 构建（Vite）。

**Spec:** 更新 `docs/specs/tauri-renderer-host.md`（`window.mode` 的注入口径、验收台与产品页的
并存方式、以及本任务承接/移交的义务清单）。纪律：`docs/specs/clean-room-policy.md`——
**函数体不算接口面**；`packages/desktop/src/**` 与 `packages/ui/src/**` 是上游衍生，只读接口面。

## Global Constraints

- **仍然不改 renderer 源码**：`git diff -- packages/desktop/src/renderer/**` 必须为空。本任务只动
  **构建接线与注入**，不动 renderer 的实现体——Task 0 的 spike 与 Task 1/2 都是靠这条成立的。
- **Windows only**（Ruling 20）；语言政策（Rust/TS/HTML/CSS；脚本 `.ts`、测试 `.mts`）；
  `oxlint max-lines` 上限 400。
- **数据隔离**：本地跑与测试**绝不**碰用户真实 `~/.mode`；沿用「`MODE_DATA_BASE_DIR` 是 base、
  根 = `join(base, ".mode")`」的已修正口径，并沿用空 base 即拒绝的保证。
- **体积要按拆分后的两份预算分别记**（Ruling 26）：本任务把**产品 UI 资产**加进来，这会让
  「核心」那一份显著变大——**必须实测并更新基准**，不能沿用验收台时代的 4.87 MiB。
- **每个任务结束**：`node scripts/rust-cargo.ts clippy --workspace --all-targets -- -D warnings`
  零告警、可跑用例全绿、`pnpm verify:pre-push` 通过；新用例要落在 CI 覆盖到的入口
  （`cargo test -p sidecar-host`、`pnpm --filter @mode/desktop-tauri test`）。

---

## 决策记录（开工前先读）

### DR-T3-1：`window.mode` 由壳的 initialization script 注入（复用 M1 的 shim）

renderer 启动时读 `window.mode`（Electron 线由 **preload** 提供）。Tauri **没有 preload /
`contextBridge`**，所以必须由**页面加载前**的注入来做——这正是 Task 0 spike 已经建成的那套机制
（`initialization_script`），且 M1 已经为此写了适配层
（`packages/ui/src/platform/tauri-invoke-shim.ts` + `createTauriPlatformService`，含
「未实现的方法显式抛 `not_implemented_until_m3`」的口径）。

**决策**：`window.mode` 由同一个 init script 注入，内容来自 M1 的 shim；**不引入 preload 等价物**、
**不改 renderer**。**代价**：注入顺序成为正确性的一部分（必须在 renderer 第一次读 `window.mode`
之前完成）——这条要有断言，不能靠「一般是够快的」。

### DR-T3-2：产品页与验收台并存，验收台降为**开发用**页面

M1 的命令面验收台是**回归工具**（10 条命令 + 深链面板），不是产品 UI。本任务把产品 UI 接上之后，
**不要把验收台删掉**：它是命令面回归与对拍的工具。做法：产品页为默认入口，验收台保留为**开发/诊断
入口**（构建产物里可留，或在 dev 构建里启用——由实施者按体积与安全权衡后定，并写进 spec）。

**代价（若判错）**：若验收台被无条件打进发布产物，它会多占体积并把内部命令面暴露给用户；
若被删掉，命令面就失去唯一的可视化回归面。

### DR-T3-3：本任务承接的既有义务（不承接的必须写明移交）

Task 1/2 的评审登记了几条**属于 Task 3/4** 的义务，本任务必须逐条处置：
① **阶段精确的失败原因送达渲染层**（现在 Host 知道 `sql_failed`/`preparing_host_storage`，
但渲染层只拿到粗粒度的 `transport_closed`/`starting`）；② **`sequence` 归属**（Host 与壳各有一个
从 0 起算的计数器，合流时会互相丢弃）；③ `disk` 摘要（现在恒 `[]`）。
**处置规则**：凡本任务**能用得上**的（①③ 会直接影响「用户看到的失败信息」）就在本任务做；
用不上的（② 在没有合并流之前不需要）就明确写「移交 Task 4，理由是…」，**不允许**留成沉默的缺口。

---

### Task 3: 真实产品 UI 挂载

**Files:**
- Modify: `apps/desktop-tauri/src-tauri/src/**`（init script 内容：加 `window.mode` 注入）
- Modify: `apps/desktop-tauri/vite.config.ts`、`tauri.conf.json`（`frontendDist` 指向真实 renderer 产物）
- Create: `apps/desktop-tauri/test/windowModeInjection.test.mts`
- Create: `apps/desktop-tauri/scripts/build-renderer-for-tauri.ts`（若需要一层构建接线）
- Modify: `docs/specs/tauri-renderer-host.md`

**Interfaces:**
- Consumes: Task 1/2 的 `HostHandle`、端口通道、真服务面；M1 的 `tauri-invoke-shim`。
- Produces: `window.mode` 在 renderer 首次访问前就位的保证（有断言）；
  指向真实 renderer 的 `frontendDist`；更新后的「核心」体积基准。

- [ ] **Step 1: 摸清 renderer 的构建产物与 `window.mode` 的消费点（只读接口面）**

确认 `packages/desktop` 的 renderer 构建输出在哪、`frontendDist` 该指向什么，以及 renderer 在
启动链的**哪一步**首次读 `window.mode`（只读接口面/类型声明，不读实现体）。把结论写进 spec。
Expected: spec 里出现「产物路径 + 首次消费点」两句可核对的陈述。

- [ ] **Step 2: 注入 `window.mode`（TDD，RED 先行）**

写 `windowModeInjection.test.mts`：断言注入的 shim 满足 `IPlatformService` 的 M1 切片，
且未实现的方法**显式抛** `not_implemented_until_m3`（沿用 M1 口径，不静默返回假值）；
并断言「renderer 首次访问前就位」这个顺序性质（可用一个在页面里先读后断言的探针）。
Run: `pnpm --filter @mode/desktop-tauri test`
Expected: **先红**（注入不存在），实现后绿。

- [ ] **Step 3: 把 `frontendDist` 指向真实 renderer 产物（DR-T3-2）**

产品页为默认入口；验收台按 DR-T3-2 的决定保留为开发/诊断入口，并在 spec 写明它在**发布产物里
是否存在**。**体积实测并更新「核心」预算基准**（Ruling 26 的两份预算）。
Expected: 构建通过 + 体积数字有命令与输出。

- [ ] **Step 4: 端到端——真实产品 UI 至少一个业务面可用**

opt-in 真机跑：壳起来 → 真 startup 放行 → **真实产品 UI 渲染** → 至少一个真实业务面走通
（不是「能显示」）。原始输出**入仓**（沿用 spec §10 的证据登记做法）。
Expected: 记录原始输出；渲染层源码 diff 为空。

- [ ] **Step 5: 处置 DR-T3-3 的三条义务 + 提交**

逐条：做掉 ①（阶段精确失败原因送达渲染层）与 ③（`disk` 摘要，若渲染层确实消费），
② 明确移交并写理由。

```bash
git add apps/desktop-tauri docs/specs/tauri-renderer-host.md
git commit -m "feat(tauri): M3 Task 3——真实产品 UI 挂载与 window.mode 注入"
```

---

## 阶段验收

1. 壳渲染**真实产品 UI**，且至少一个真实业务面端到端可用（有入仓的原始输出）；
2. `window.mode` 在 renderer 首次访问前就位，且有断言（不是靠时序运气）；
3. **renderer 源码未被修改**（diff 为空），验收台按 DR-T3-2 保留并可运行；
4. 「核心」体积基准按拆分后的口径**实测更新**（含产品 UI 资产）；
5. DR-T3-3 三条义务逐条处置（做掉或明确移交并写理由）；
6. `clippy -D warnings` 零告警、可跑用例全绿、`pnpm verify:pre-push` 通过。

## 风险

| 风险 | 对冲 |
| --- | --- |
| 注入时序不稳（`window.mode` 晚于 renderer 首次访问） | DR-T3-1 把顺序做成断言；真机跑必须覆盖冷启动 |
| 为了让它跑起来而顺手改 renderer | 硬约束 + 收尾 `git diff` 检查；发现必须改就**停下来回报**（会改到 M4 计划） |
| 验收台误入发布产物（体积 + 暴露内部命令面） | DR-T3-2 明确决定并写进 spec，体积实测会暴露它 |
| 产品 UI 资产让「核心」预算大幅上升而被误读成回归 | Ruling 26 已把门禁拆两份；本任务只**更新基准**，不重新谈判门禁 |
| 既有义务被当成「以后再说」 | DR-T3-3 逐条处置规则：做掉或写明移交理由，不留沉默缺口 |
