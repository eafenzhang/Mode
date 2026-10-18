# M3 Task 2：真服务面 + database-startup 通道 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `subagent-driven-development`（推荐）或
> `executing-plans` 逐任务实现。步骤用 `- [ ]` 复选框跟踪。
>
> **开工前必读**：阶段计划 `docs/plans/2026-10-16-tauri-m3-ecosystem.md`（DR-M3-1/2/3 与
> Global Constraints）、Task 1 的实现 spec `docs/specs/tauri-renderer-host.md`（Host 契约、
> **§已知危险**、§10 的证据登记）、传输路线 `docs/plans/2026-10-16-m3-port-transport-spike.md` §4。
> Task 1 的交接产物是 `crates/sidecar-host` + `packages/desktop/src/host-standalone/`。

**Goal:** 把 Task 1 的**空壳**换成**真服务面**：sidecar 里跑起真实的 `packages/services`，
用真实的 database-startup 状态驱动渲染层的闸门——使产品 UI 挂载后**功能真的可用**，
而不只是「挂上了」。

**Architecture:** Task 1 已验证「壳 → Host → 端口 → 渲染层」这条链能放行；本任务只替换两端的
内容物：`stubAccessor` → 真 `IServiceAccessor`；Task 1 的**合成** `phase: "ready"` → 由运行时
真实的 `startup/storageState` 帧映射出的 `DatabaseStartupState` 流。壳体与传输层**不改**
（若必须改，说明 Task 1 的抽象选错了，停下来回报）。

**Tech Stack:** 同 Task 1（Rust 1.99 + tauri 2.12 + Node 24 + `packages/services` + `packages/rpc`）。
新增关注点：`node:sqlite`、`node-pty` 等**原生依赖**在 sidecar 里的可用性，以及 sidecar 的打包形态。

**Spec:** 更新 `docs/specs/tauri-renderer-host.md`（真服务面的启动语义、startup 映射口径、
原生依赖与打包形态、以及「哪些由自动化覆盖」的更新）。纪律：`docs/specs/clean-room-policy.md`
——**函数体不算接口面**；`packages/services/**` 与 `packages/desktop/src/**` 是上游衍生，
只读接口面。

## Global Constraints

- **不改渲染层**（沿用 Task 1）：`git diff -- packages/desktop/src/renderer/**` 必须为空。
- **数据根口径（Task 1 的 I2 修正）**：`MODE_DATA_BASE_DIR` 是 **base**，数据根 = `join(base, ".mode")`。
  沿用两侧已修正的实现，**不要再解释成根**。
- **`listen` 抛错会杀死 Host**（Task 1 的 C1，已写进 spec §已知危险）：任何真实服务若在 `listen`
  里抛，就会打死 Host，而壳侧分发**没有** try/catch。**本任务第一件事就是验证真服务不会这样**；
  若会，修的位置在壳/传输侧，不是逐个服务兜。
- **数据隔离**：测试与本地跑**绝不**碰用户真实 `~/.mode`；且**不许**出现「忘了设 data_root 就
  静默指向真实根」的路径（Task 1 已把空 base 改为 `InvalidConfig`，沿用该保证）。
- **Windows only**（Ruling 20）；语言政策（Rust/TS/HTML/CSS，脚本 `.ts`、测试 `.mts`）；
  `oxlint` `max-lines` 上限 400（此前已有文件在 396 处擦线）。
- **每个任务结束**：`node scripts/rust-cargo.ts clippy --workspace --all-targets -- -D warnings`
  零告警、可跑用例全绿、`pnpm verify:pre-push` 通过。CI 现已跑 `-p sidecar-host` 与
  `pnpm --filter @mode/desktop-tauri test`（Task 1 的 I4 接线），**新用例要落在这些入口覆盖得到的地方**。

---

## 决策记录（开工前先读）

### DR-T2-1：先证原生依赖，再写任何服务接线

`packages/services` 依赖 `node:sqlite`（`DatabaseSync`，需较新 Node）与 `node-pty`（ABI 敏感的原生
模块），另有 CUA 相关的原生资产走 env 注入。**这些在 Tauri 的 sidecar 环境下能否加载，决定整条
路线成不成立**（DR-M3-1 选了 Node sidecar 正是为复用它们）。

因此 Step 1 **只做这一件事**：在 sidecar 进程里 import 并实例化它们，把结果记成用例。
**代价（若判错）**：若原生模块在 sidecar 里不可用，Node sidecar 的前提就不成立，要回到
DR-M3-1 重新决策（那是远大于本计划的改动）——所以这件事必须在最前面，而不是在接了二十个
服务之后才发现。

### DR-T2-2：startup 映射的正确方向是「运行时 → 渲染层」

真源是运行时 emit 的 `startup/storageState` 帧（形状：`{schemaVersion, attemptId, sequence,
databaseId, databaseKind, phase, migration?, elapsedMs}`，M2 Task 0 实测并已录进
`packages/shared/test/fixtures/agent-protocol/`）。渲染层要的是 `DatabaseStartupState`
（`phase` 取值不同：`starting | preparing_host_storage | preparing_session_storage |
starting_services | ready | failed`，且有 `startupId` 与 `databaseStartupId` 的匹配约束）。

**决策**：映射是**一层显式的翻译**，落点放在 sidecar 侧（`host-standalone`），并**由夹具驱动**
——复用 M2 已录制的 `startup/storageState` 帧作为输入，断言输出符合 `DatabaseStartupState` 的
zod schema。**不要**把两者的 phase 名硬凑成同名，也不要让渲染层适配。**代价**：多一层翻译
（但这是唯一能同时满足「真源在运行时」与「渲染层不改」的做法）。

### DR-T2-3：sidecar 的打包形态在本任务定，并实测体积

安装包要带 sidecar（DR-M3-2）。Task 1 只跑了源码形态（tsx）。本任务要决定并落地**打包**形态
（bundle 成单文件还是随包带 node_modules），**并实测体积**——这个数字是 Task 5 体积门禁的输入。
**代价（若判错）**：体积或启动开销超预期时，Task 5 会以「门禁不达标」的形式发现，届时要回头改
打包策略而不是改门禁。

### DR-T2-4：重启策略（Task 1 已登记为「刻意缺失」）

Task 1 把「崩溃重启策略」登记为**故意不做**并写明理由。本任务必须**处置**它：要么实现最小策略
（例如「崩溃后重启一次，再崩溃则推 `failed` 并停」），要么继续登记为不做并说明为什么在 Task 3
（挂载真实 UI）之前可以不做。**不允许**让它继续以「计划提过、规格没写、代码没有」的形态存在。

---

### Task 2: 真服务面 + startup 通道

**Files:**
- Create: `packages/desktop/src/host-standalone/serviceAccessor.ts`（真 `IServiceAccessor` 装配）
- Create: `packages/desktop/src/host-standalone/startupMapping.ts`（`startup/storageState` → `DatabaseStartupState`）
- Create: `packages/desktop/src/host-standalone/nativeDeps.test.mts`（DR-T2-1 的验证）
- Modify: `packages/desktop/src/host-standalone/index.ts`（用真 accessor 替换 stub）
- Modify: `crates/sidecar-host/**`（仅当打包形态或重启策略需要）
- Modify: `docs/specs/tauri-renderer-host.md`
- Test: `apps/desktop-tauri/test/*.test.mts`（真实服务可用性）、`packages/desktop/tests/host-standalone.test.mts`

**Interfaces:**
- Consumes: Task 1 的 `HostHandle`、端口通道、`DatabaseStartupState` 播发路径；
  M2 录制的 `startup/storageState` 协议夹具。
- Produces: 真实 `IServiceAccessor`（由 `packages/services` 装配，不再有 stub 方法）；
  由真实 startup 帧映射出的状态流；打包后的 sidecar 产物与它的体积数字。

- [x] **Step 1: 先证原生依赖（DR-T2-1）**
  〔2026-10-19 核对：`apps/desktop-tauri/test/nativeDeps.test.mts` 已通过（同批 35 pass）。〕

写 `nativeDeps.test.mts`：在 sidecar 的运行环境里 import 并实例化 `node:sqlite` 与 `node-pty`（以及
CUA 相关资产按 env 注入的路径），断言可用。
Run: `pnpm --filter @mode/desktop-tauri test`
Expected: **先红后绿**；若红且不可修，**停下来回报**（这推翻 DR-M3-1）。

- [x] **Step 2: 起真 services 并装配 `IServiceAccessor`**
  〔2026-10-19 核对：`packages/desktop/src/host-standalone/serviceAccessor.ts` 已装配；`realServices.test.mts` 已在仓。〕

`serviceAccessor.ts` 按 `packages/services` 的**公共入口**装配全部服务（不 deep import）。
逐个服务的接线以小步提交，每步都有用例；**先验证没有任何服务的 `listen` 会抛**（Global Constraints
里的那条危险），并把这个检查做成一次性断言而不是靠人记得。
Expected: 真实 accessor 可 `call`，且 `listen` 订阅不杀 Host。

- [x] **Step 3: startup 映射（DR-T2-2，夹具驱动）**
  〔2026-10-19 核对：`startupMapping.ts` 与 `apps/desktop-tauri/test/harness/` 夹具用例已在仓。〕

`startupMapping.ts` 用 M2 录制的真实帧做输入，断言输出符合 `DatabaseStartupState` 的 schema；
覆盖 `starting`、迁移中、`ready`、`failed`（含 `errorCode`）四条路径。
Expected: 夹具用例全绿；**闸门由真实状态驱动**（不再有 Task 1 的合成 ready）。

- [ ] **Step 4: 端到端（opt-in 真 webview）+ 失败语义**
  〔2026-10-19 核对：`webviewRoundTrip.test.mts` 已建（默认 skip）；真机启动/失败路径仍待人工跑。〕

跑一次真机：壳起来 → 真 startup → 闸门放行 → **至少一个真实业务面可用**（不是「挂上了」）；
并验证失败路径：startup 失败时渲染层收到 `failed` + `errorCode`（回忆 Task 1 的订正：
`starting` 送达后计时器被拆除，**`failed` 是唯一的失败信号**）。
Expected: 原始输出**入仓**（照 Task 1 spec §10 的证据登记做法，不要只写在报告里）。

- [x] **Step 5: 打包形态与体积（DR-T2-3）**
  〔2026-10-19 核对：`build-host-sidecar.ts` 已交付；spec §13 记录 bundle 4.87 MiB 与安装包预算。〕

落地 sidecar 的打包形态，实测体积并把数字写进 spec + 交给 Task 5。
Expected: 体积数字有出处（命令 + 输出），不是估计。

- [x] **Step 6: 重启策略（DR-T2-4）+ 提交**
  〔2026-10-19 核对：spec §14 正式登记「不重启」及理由/代价；提交已存在。〕

按 DR-T2-4 做决定并落地/登记。

```bash
git add packages/desktop/src/host-standalone packages/desktop/tests crates/sidecar-host apps/desktop-tauri docs/specs/tauri-renderer-host.md
git commit -m "feat(tauri): M3 Task 2——真服务面与 startup 通道，产品面可用"
```

---

## 阶段验收

1. 真实 `IServiceAccessor` 装配完成，UI 挂载后**至少一个真实业务面可用**（不是「能显示」）；
2. 闸门由**真实** startup 状态驱动；失败时渲染层收到 `failed` + `errorCode`；
3. 原生依赖（`node:sqlite`/`node-pty`）在 sidecar 里**实测可用**，并有自动化用例；
4. 没有任何服务因 `listen` 抛错而杀死 Host（有断言，不是靠人记得）；
5. 数据隔离：测试与本地跑不碰真实 `~/.mode`，且无「忘设就指向真实根」的路径；
6. sidecar 打包形态落地且**体积实测**（数字交给 Task 5）；
7. 渲染层未修改；`clippy -D warnings` 零告警、`pnpm verify:pre-push` 通过、新用例落在 CI 覆盖到的入口；
8. 重启策略按 DR-T2-4 落地或明确登记。

## 风险

| 风险 | 对冲 |
| --- | --- |
| **原生依赖在 sidecar 里不可用** | DR-T2-1 把它放在 Step 1；失败即回报并按 DR-M3-1 重新决策 |
| 真实服务在某处 `listen` 抛错 → Host 死 | Step 2 把「无服务因 listen 抛错而杀 Host」做成断言；真出现时修壳/传输侧 |
| startup 映射把 phase 硬凑成同名，掩盖语义差异 | DR-T2-2：显式翻译 + 夹具驱动，覆盖四条路径 |
| 打包后体积/启动开销超预期 | DR-T2-3 在本任务实测，而不是留到 Task 5 撞门禁 |
| 真实服务开始写用户数据，而数据根语义又错了 | Global Constraints 固化 base 语义；沿用 Task 1 的空 base 拒绝 |
| 把 opt-in 真机跑的结论只写进报告 | Step 4 要求原始输出入仓（Task 1 已建立这个做法） |
