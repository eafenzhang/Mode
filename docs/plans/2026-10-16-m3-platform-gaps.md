# M3 Task 4：平台面缺口收口 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `subagent-driven-development`（推荐）或
> `executing-plans` 逐任务实现。步骤用 `- [ ]` 复选框跟踪。
>
> **开工前必读**：阶段计划 `docs/plans/2026-10-16-tauri-m3-ecosystem.md`（Global Constraints、
> Ruling 20 的 Windows 范围、体积门禁拆分）、Host 契约 `docs/specs/tauri-renderer-host.md`
> （§15 义务登记、§19 clean-room 登记、§20 开放项）、M1 平台面 spec
> `docs/specs/tauri-platform-commands.md`（**§1 命令切片表**、**§5 未完成项**、§4 验收判据）。

**Goal:** 关掉 M1 §5 里登记给 M3 的平台面缺口，并把 Task 3 移交的两条义务处置掉——使 Tauri 版
在平台命令面上不再有「契约允许但恒返回占位值」的成员。

**Architecture:** M1 已经建好命令面（`crates/platform/src/commands/**` 的 10 条命令 + 切片表宏 +
`rename` 名字契约）。本任务**只替换其中几条的实现体**，命令名/签名/契约不动；深链单实例是唯一
需要动窗口模型的一项，它落在壳侧（`apps/desktop-tauri/src-tauri/**`）。

**Tech Stack:** 同前（Rust 1.99 + tauri 2.12 + Windows 原生 API），外加一个带超时/大小上限的
HTTP 取回面（`sourceUrl` 分支）。

**Spec:** 更新 `docs/specs/tauri-platform-commands.md`（§1 的口径从「部分实现」改为「已实现」、
§5 的对应行勾掉、深链单实例的窗口模型口径写清楚）。纪律：`docs/specs/clean-room-policy.md`
——**函数体不算接口面**；`packages/shared/src/platform.ts` 只读接口面。

## Global Constraints

- **不改渲染层**：`git diff -- packages/desktop/src/renderer/**` 必须为空。
- **Windows only**（Ruling 20）：`getApplicationIcon` 的 macOS 半边**不做也不验**（没有 macOS 构建，
  写了也无法验证）——但要在 §5 里标明「未实现且本阶段不验」，不许沉默省略。
- **命令名与切片表逐字不变**：M1 的 `rename` 契约与守卫（`crates/platform/tests/commands.rs`）是
  跨端契约，本任务只换实现体。
- **数据隔离 / 语言政策 / `oxlint max-lines` 400 / 本机 cargo 走 `scripts/rust-cargo.ts`**：
  沿用既有口径。
- **每个任务结束**：`clippy --workspace --all-targets -- -D warnings` 零告警、可跑用例全绿、
  `pnpm verify:pre-push` 通过；新用例落在 CI 覆盖到的入口。

---

## 决策记录（开工前先读）

### DR-T4-1：深链单实例转发是**窗口模型**决定，不是「加个锁」

现状（M1 §5 已登记）：Windows 上应用已在运行时点 `mode://` 链接会**起新进程 → 新窗口**。
这与 `AGENTS.md` 的「每个窗口一个 window-scoped Local Host、本地 workspace 共享该 Host」相冲：
新进程意味着第二个 Host、第二份 workspace 状态。

**决策**：走**单实例**：第二个进程只做两件事——把 argv 里的深链交给已在运行的实例，然后**不建窗口、
不建 Host** 直接退出。转发通道复用已建成的端口/事件通道（Task 1/2 的成果），**不另造 IPC**。
**代价（若判错）**：单实例会改变窗口生命周期语义（例如「多窗口」在 M4 的 UI 替换里若要支持，
需要重新设计这一层）；所以本任务只做「同一个 workspace 不重复起 Host」，并把该约束写进 spec。

### DR-T4-2：`getApplicationIcon` 走原生解码，但**只做 Windows**

契约允许返回 `null`，M1 恒返回 `null`。本任务在 Windows 上取 exe 的图标并编码为位图数据 URL。
**代价**：Windows 的实现要用 GDI/Shell API，是本任务里唯一涉及原生资源解码的部分；macOS 半边
按 Ruling 20 不做（写明未实现且不验）。

### DR-T4-3：`saveFile` 的 `sourceUrl` 需要一个**有上限**的 HTTP 取回面

M1 对该分支明确返回 `save_file_source_url_not_implemented_until_m3`（不静默写空文件）。本任务实现它，
但**必须带超时与大小上限**，并把两者做成可配置项/常量 + 用例——否则这是一个「任意远端让它写任意大
文件」的入口。

### DR-T4-4：承接 Task 3 移交的两条义务

① **`sequence` 归属**：Host 与壳各有一个从 0 起算的计数器，合流时会互相丢弃。Task 3 移交的理由
是「壳仍是唯一发布者」——本任务若引入任何合并流（深链转发会经事件通道），**必须先把归属定下来**，
否则就继续移交并写明「谁在什么条件下必须定」。② **`disk` 摘要**：全仓无消费者；本任务若无消费者
就继续移交，但要**在 spec 的开放项里保持可见**。

---

### Task 4: 平台面缺口 + 两条移交义务

**Files:**
- Modify: `crates/platform/src/commands/icon.rs`（原生图标解码，Windows）
- Modify: `crates/platform/src/commands/dialog.rs`（`saveFile` 的 `sourceUrl` 分支 + HTTP 上限）
- Modify: `crates/platform/src/commands/window.rs` / `deeplink` 接线（其余订阅型方法，若依赖已就绪）
- Modify: `apps/desktop-tauri/src-tauri/**`（单实例：第二进程转发后退出）
- Modify: `docs/specs/tauri-platform-commands.md`
- Test: `crates/platform/tests/*`、`apps/desktop-tauri/test/*.test.mts`

**Interfaces:**
- Consumes: M1 的切片表与 `rename` 契约、Task 1/2 的事件/端口通道。
- Produces: 三条命令的真实实现（`getApplicationIcon` / `saveFile(sourceUrl)` / 深链单实例），
  以及 `sequence` 归属与 `disk` 摘要的明确处置。

- [ ] **Step 1: 单实例（DR-T4-1）——先写失败用例**

写用例断言：已运行实例在场时，第二次启动**不产生第二个 Host、不新建窗口**，且深链**到达**第一实例的
渲染层。先红（现在是起新进程）。
Expected: RED 输出记下来（当前行为 = 新进程/新窗口）。

- [ ] **Step 2: 实现单实例转发**

第二进程检测已在运行的实例 → 转发深链 → 退出。复用既有事件通道；**不新造 IPC**。
Expected: Step 1 的用例绿；并断言退出码与「不留孤儿」。

- [ ] **Step 3: `getApplicationIcon` 原生解码（DR-T4-2，Windows）**

按 M1 §1 的契约返回 `IconInfo | null`；真实取值失败时返回 `null`（契约允许），**不抛**。
Expected: 用例断言「返回非 null 且是数据 URL」，以及「取不到时返回 null 而不是抛」。

- [ ] **Step 4: `saveFile` 的 `sourceUrl`（DR-T4-3）**

实现取回 + 落盘；**超时与大小上限**做成常量 + 用例（超限明确失败，不静默写半截）。
Expected: 上限用例绿；并保留 M1 的「取消返回 `canceled: true`」语义。

- [ ] **Step 5: DR-T4-4 两条义务 + 收尾**

按 DR-T4-4 处置 `sequence` 与 `disk`；更新 §1/§5 的口径（含 macOS 半边的「未实现且不验」）。

```bash
git add crates/platform apps/desktop-tauri docs/specs/tauri-platform-commands.md
git commit -m "feat(tauri): M3 Task 4——平台面缺口收口（图标/取回/深链单实例）"
```

---

## 阶段验收

1. §5 登记的四项（原生图标、`sourceUrl`、其余订阅型、**深链单实例**）逐条关闭或**明确登记为不做并写理由**；
2. 深链在应用已运行时**不再起新进程/新窗口**，且深链确实到达已有实例的渲染层（有断言）；
3. `saveFile(sourceUrl)` 有超时与大小上限用例；
4. `sequence` 归属与 `disk` 摘要**处置完毕**（做掉或写明移交与条件），不留沉默缺口；
5. 命令名/签名/`rename` 契约未变（M1 的守卫仍绿）；渲染层未修改；
6. `clippy -D warnings` 零告警、可跑用例全绿、`pnpm verify:pre-push` 通过。

## 风险

| 风险 | 对冲 |
| --- | --- |
| 单实例改动窗口模型，波及 M4 的多窗口设想 | DR-T4-1 明确写下该约束与代价；只做「不重复起 Host」，不顺手重构窗口管理 |
| `sourceUrl` 成为无上限的写盘入口 | DR-T4-3 把超时与大小上限作为**必须项**并配用例 |
| 原生图标解码在 CI 上不可验（无桌面） | 用例只断言契约形状（数据 URL / null），真实取值靠 opt-in 真机跑并把输出入仓 |
| 两条移交义务被继续「移交」到无人 | DR-T4-4 要求写明**谁在什么条件下必须定**，并在 spec 开放项里保持可见 |
| macOS 半边被沉默省略 | Global Constraints 要求写明「未实现且本阶段不验」，同 Ruling 20 的登记做法 |
