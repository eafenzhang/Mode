# M3 Task 5：Windows 出包、两份体积预算与冷启动实测 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `subagent-driven-development`（推荐）或
> `executing-plans` 逐任务实现。步骤用 `- [ ]` 复选框跟踪。
>
> **开工前必读**：阶段计划 `docs/plans/2026-10-16-tauri-m3-ecosystem.md`（**体积门禁拆分登记**一节、
> Ruling 20 的 Windows 范围、Ruling 26）、Host 契约 `docs/specs/tauri-renderer-host.md`
> （§13 的体积数字与它们测在**哪张脸**上、§17 的打包形态、§20 开放项）、M1 平台面 spec
> `docs/specs/tauri-platform-commands.md` §8（出包与体积基线的历史口径）。

**Goal:** 产出**真实可安装的 Windows 产物**（产品 UI + sidecar + Node 运行时），在两份拆分预算上
**实测并给出数字**，测出**冷启动**，并把 M5 切换发布线所需的一切整理成交接清单。

**Architecture:** 本任务不改产品逻辑，只做**打包、测量与交接**：把 Task 1–4 已经跑通的壳 +
Host sidecar + 真实产品 UI 打成安装包，然后**在门禁真正使用的那个面上**量它。

**Tech Stack:** tauri 2.12 的 NSIS bundler、既有的 `apps/desktop-tauri/scripts/build-host-sidecar.ts`
与 `scripts/tauri-bundle-report.ts`。

**Spec:** 更新 `docs/specs/tauri-renderer-host.md` 的体积与冷启动章节，以及
`docs/specs/tauri-platform-commands.md` §8 的体积基线表。纪律：clean-room 同前（本任务大概率只读
构建脚本与配置，不读上游实现体）。

## Global Constraints

- **Windows only**（Ruling 20）：只出 Windows 产物；macOS/Linux 的延后与代价已在阶段计划登记，
  本任务**不要**顺手解冻。
- **测量口径必须写清楚**：历史上这个数字出过三次口径事故（文件求和 vs 安装包字节、含不含壳 exe、
  未压缩 vs 压缩），所以本任务**每个数字都必须标注它测在哪张脸上**，并给出命令与原始输出。
- **不改产品逻辑**：本任务只动打包与测量；若为了让产物跑起来必须改逻辑，**停下来回报**。
- **数据隔离**：安装/运行验证**不得**碰用户真实 `~/.mode`（用隔离 base）。
- **语言政策**：新增脚本 `.ts`；`oxlint max-lines` 400。
- **每个任务结束**：`clippy -D warnings` 零告警、可跑用例全绿、`pnpm verify:pre-push` 通过。

---

## 决策记录（开工前先读）

### DR-T5-1：所有体积数字都测在**NSIS 安装包字节**这张脸上

门禁的口径是**安装包字节**（`tauri-platform-commands.md` §8.2 的历史约定）。而 Task 2/3 报的
96.79 MiB / 102.61 MiB / 54.41 MiB 分别是**载荷求和**、**传递闭包**、**未压缩文件字节和（不含壳 exe）**
——三张不同的脸。继续这样比，等于每次都在换尺子。

**决策**：本任务的两份预算（(a) 核心 / (b) Node 运行时 + sidecar externals）**都**在安装包字节面上
给出，并**同时**给出各组成部分的未压缩尺寸作为诊断信息（标注为「诊断，不是门禁口径」）。
两套数字并存没问题，**混用才是问题**。

**代价（若判错）**：压缩后的数字与代码/资产字节数没有稳定倍数关系（`.node` 约 4.3:1，文本更高），
所以「这次能不能过门禁」只能靠实测回答——这也正是本任务存在的理由。

### DR-T5-2：剪枝要同时报「原始省了多少」与「压缩后省了多少」

Task 2 已指出：剪掉 node-pty 的跨平台 prebuilds 省 28.17 MiB **原始**，压缩后只省约 6.6 MiB。
**决策**：任何剪枝都按这两个数一起报，并明确「门禁看的是后者」。不要只报原始值——那会让
一次 28 MiB 的「优化」在门禁上看起来毫无作用、而另一次小改动看起来很大。

### DR-T5-3：门禁数字本身**待人定**，本任务只产出数字与建议

Ruling 26 把门禁拆成两份，但 (a) 的 ≤30MB 在 Task 3 实测（核心 54.41 MiB，且**不含壳 exe**）
之后已明显不可达。**决策**：本任务**不自行改门禁数字**，而是产出——(a)/(b) 的实测值、
release-exe 与冷启动的实测值、以及一份明确的建议（例如「(a) 应设为 X」或「接受超出并改写验收」）
连同**唯一的 ≤30MB 出路**（Host/services 用 Rust，即 DR-M3-1 拒绝的慢路径）。
**代价（若判错）**：门禁会在一段时间里保持一个已知不可达的数字；但自行改数会让「门禁」失去意义，
两者相比宁可让数字挂着等人拍。

### DR-T5-4：冷启动的口径要先定，再测

Task 2/3 只跑过 debug 构建，**冷启动从未测过**，而它是 DR-M3-1 明确列出的代价之一。
**决策**：先写下口径（什么算冷启动：首次启动 / 无缓存 / 含 Node runtime 与 Host 就绪与闸门放行的
**哪一段**；在什么机器上测；采样几次、取什么统计量），再测。**不许**先测再随口解释测了什么。

---

### Task 5: Windows 出包 + 两份预算 + 冷启动

**Files:**
- Modify: `apps/desktop-tauri/scripts/build-host-sidecar.ts`（剪枝与体积报告）
- Modify: `apps/desktop-tauri/src-tauri/tauri.conf.json`（若产物配置需要）
- Modify: `scripts/tauri-bundle-report.ts`（两份预算的口径）
- Modify: `docs/specs/tauri-renderer-host.md`、`docs/specs/tauri-platform-commands.md` §8
- Create: `docs/plans/2026-XX-XX-m5-release-cutover-input.md`（或写进阶段计划的一节）

**Interfaces:**
- Consumes: Task 1–4 的壳 / sidecar / 真实产品 UI / 平台面实现。
- Produces: 真实 Windows 安装包、**两份预算的实测值（安装包字节面）**、冷启动实测值、
  M5 的交接清单。

- [ ] **Step 1: 先写口径，再动手（DR-T5-1 / DR-T5-4）**

把「两份预算各包含什么、测在哪张脸上」与「冷启动的起止点与采样方式」写进 spec。
Expected: 两句可核对的陈述，且**在测之前**就存在。

- [ ] **Step 2: 产出真实 Windows 安装包**

用与 CI 同款的命令出包（当前 CI 跑的是不含 sidecar 的验收台配置——**先确认产物里真的有
sidecar 与产品 UI**，别把验收台的包当成交付物）。
Expected: 产物路径 + `sha256` + 字节数。

- [ ] **Step 3: 两份预算实测（安装包字节面）+ 剪枝**

按 DR-T5-1 报两份预算；按 DR-T5-2 同时报剪枝的原始/压缩收益。
Expected: 每个数字都有命令与原始输出；剪枝前后各一组。

- [ ] **Step 4: 冷启动实测（按 Step 1 的口径）**

Expected: 采样值与统计量；debug 与 release **都要**（debug 只作对照）。

- [ ] **Step 5: 安装后验证（隔离 base）**

装到隔离数据根后：产品 UI 能起、Host 就绪、闸门放行、**至少一个真实业务面可用**；
并覆盖 Task 3 登记的那条**窗口化发布构建下的平台面可见性**缺口（§20 开放项 1）。
Expected: 原始输出入仓（沿用证据登记做法）。

- [ ] **Step 6: M5 交接清单 + 提交**

清单至少含：产物名与安装方式、更新链需要什么（签名密钥 / `latest.json` / 三平台演练）、
以及「发版前必须人工跑的事」（Task 1 的 DR-T1-2 划出的那批）。

```bash
git add apps/desktop-tauri scripts docs/specs docs/plans
git commit -m "feat(tauri): M3 Task 5——Windows 出包、两份体积预算与冷启动实测"
```

---

## 阶段验收

1. 真实 Windows 安装包产出，内含**产品 UI + sidecar**（不是验收台配置的包）；
2. 两份预算在**安装包字节面**上给出实测值，每项带命令与原始输出，并标注算在哪张脸上；
3. 剪枝同时报原始与压缩收益（DR-T5-2）；
4. 冷启动按**预先写下的口径**实测，debug 作对照，release 为结论；
5. 安装后（隔离 base）产品 UI 可用，且覆盖窗口化发布构建下的平台面可见性缺口；
6. M5 交接清单就位（产物名/安装方式/更新链所需/发版前人工项）；
7. 门禁数字**未被自行修改**，而是给出数字 + 建议 + 唯一替代路径（DR-T5-3）；
8. `clippy -D warnings` 零告警、可跑用例全绿、`pnpm verify:pre-push` 通过。

## 风险

| 风险 | 对冲 |
| --- | --- |
| **又用量错脸的数字下结论**（已出过三次） | DR-T5-1：两份预算都在门禁脸；每个数字标脸并附命令 |
| 剪枝把 node-pty 在运行时弄坏（跨平台 prebuilds 被删） | Step 3 剪枝后必须**装后实跑**一次真实终端功能；只报体积不算验过 |
| 出的是验收台配置的包，却当成产品包 | Step 2 明确要求确认产物内含 sidecar 与产品 UI |
| 冷启动口中无数（「快了/慢了」） | DR-T5-4：口径先写再测 |
| 门禁数字被顺手改掉 | DR-T5-3：只产出数字 + 建议 + 替代路径，改数归人 |
| 窗口化发布构建下的平台面不可见（Task 3 登记） | Step 5 明确覆盖该开放项，做不到就继续登记并写代价 |
