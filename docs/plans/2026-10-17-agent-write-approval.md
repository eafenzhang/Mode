# M2 Task 3：写工具（`Write` / `Edit`）与审批三态 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `subagent-driven-development`（推荐）或
> `executing-plans` 逐任务实现。步骤用 `- [ ]` 复选框跟踪。
>
> 本计划是 `docs/plans/2026-10-16-tauri-agent-runtime.md` 里 **Task 3** 的子计划，
> 按那份计划文末的约定在开工时产出。**开工前的工具面观察已完成（2026-10-17）**，
> 结论落在 `docs/specs/tauri-agent-runtime.md` §3.3——本节先把两处口径修正摊开。

**Goal:** 让 Rust 侧的 `Write` 与 `Edit` 在**行为**上与旧运行时逐条对拍通过，并把审批面
（`interaction/requestPermission` 的三条选项 + `permissionUpdates`）镜像成 Rust 类型。

## 口径修正（读代码前先读这里）

阶段计划 Task 3 的标题是「文件写与 diff 呈现（`Write` / `Edit` / `ApplyPatch`）+ 审批三态」，
接口写的是 `Decision = AllowOnce | AllowAlways | ProjectAlways | Deny`。**两处都与实测不符**：

| 计划原文 | 实测（`probe-write.mts`，2026-10-17） | 处置 |
| --- | --- | --- |
| 实现 `ApplyPatch`（含多文件与失败回滚） | `ApplyPatch` **既不在 21 条声明里、调用也返回 `tool_not_found`** | **作废**，不实现（同 Ruling 17 对 `Glob`/`Grep` 的处置） |
| `Decision` 四态 | 审批请求的 `options[]` 只有**三条**：`allow_once` / `allow_project`（`kind: allow_always`）/ `deny`；**没有**独立的全局一直允许 | 改成三态，`kind` 与 `optionId` 分开建模 |

**这条不作废但需要人裁决（D1）**：计划写「`ApprovalStore` 的数据文件与旧版同格式」，实测里
`allow_project` 的落盘是**运行时 DB 的一行**（`.mode/cli/db/db.sqlite` 的 `local_setting`，
见 spec §3.3），不是文件；且目标架构中该 DB 由 M3 的 Node Host 持有。**Rust 侧直连 sqlite
还是经 Host 代理，是架构分叉**，见本文末「待裁决」。

## Global Constraints

- **clean-room**：`apps/mode-cli/**`、`packages/services/src/**` 的**实现体禁读**——包括
  `apps/mode-cli/packages/adapters/src/storage/session-store/migrations.ts` 里 `local_setting`
  的 DDL。表结构一律用**黑盒观察**得到（本计划的探针就是干这个的），不去读那份迁移文件。
- 语言政策：新增只允许 Rust / TS / HTML / CSS。新脚本 `.ts`、新测试 `.mts`。
- 每个步骤结束：`node scripts/rust-cargo.ts clippy --all-targets -- -D warnings` 零告警 +
  本机可跑用例全绿 + `pnpm verify:pre-push` 通过。
- 对拍只比**行为**（进程输入输出、事件序列、落盘内容），不打开旧实现照着写。

---

### Step 1（**已完成，2026-10-17**）：审批三态的格式对拍

产物：`docs/specs/tauri-agent-runtime.md` §3.3（含审批请求逐字形状、三条选项、`local_setting` 行
的逐列值、累积/deny/多 cwd 四条配套事实）。复现入口：

```
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-write.mts surface
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-write.mts call
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-write.mts perm
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-write.mts project
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-write-rules.mts
```

---

### Task A：`Write` 的行为对拍

**Files:**
- Create: `crates/agent-tools/src/write.rs`
- Create: `crates/agent-tools/tests/write_parity.rs`
- Create: `packages/desktop/tests/agent-runtime-oracle/fixtures/write-*.json`（由 `record.mts` 录）

**Interfaces:**
- Consumes: `agent_tools::ToolSpec` / `registry()`（Task 2 的既有约定）、
  `agent_core::tool::{invoke, ToolContext, ToolOutcome, ToolError}`、
  `Read` 留下的 `readFileState`（「读过没有」的判据来源，Task 2 已实现）。
- Produces: `agent_tools::write::spec()` 与 `invoke` 分支。

- [ ] **Step A1：录夹具（先有 oracle 数据）**

新增 `write-new` / `write-overwrite-read` / `write-overwrite-unread` / `write-empty-args` 四条夹具，
用 `record.mts` 让旧运行时跑出来录，**不手写期望值**。

- [ ] **Step A2：写 Rust 侧对拍用例（先失败）**

`crates/agent-tools/tests/write_parity.rs` 逐条夹具断言：结果 `success`、`content` 逐字、
以及调用后的**文件快照**（`made.txt` 的字节）。三条错误路径断言 `error.type` / `message` / `detail`。

- [ ] **Step A3：实现 `Write`**

关键语义（全部以 §3.3 的观察为准）：新建成功文案 / 覆盖已读成功文案 / 覆盖未读报
`write_file_not_read`；`file_path` 相对路径按会话 cwd 解析；写入后该文件进入「已读」状态。

- [ ] **Step A4：跑通并提交**

---

### Task B：`Edit` 的行为对拍

**Files:**
- Create: `crates/agent-tools/src/edit.rs`
- Create: `crates/agent-tools/tests/edit_parity.rs`
- Create: `packages/desktop/tests/agent-runtime-oracle/fixtures/edit-*.json`
- Modify: `packages/desktop/tests/agent-runtime-oracle/probe-write.mts`（加 `edit` 探针组补观察）

**Interfaces:**
- Produces: `agent_tools::edit::spec()` 与 `invoke` 分支。

- [ ] **Step B1：先把 `Edit` 的行为观察补全**

§3.3 目前只观察到「未读就编辑 → 报错」这一条。**开工第一件事**是把这些探完并录进 §3.3：
替换必须唯一（重复串的报错文案）、`old_string` 不存在、`new_string == old_string`、
`replace_all` 的语义、成功文案、是否也写 `readFileState`、多行 `old_string` 的匹配口径。

- [ ] **Step B2：录夹具 → Step B3：写失败的对拍用例 → Step B4：实现 `Edit` → Step B5：跑通提交**

---

### Task C：审批面的 Rust 镜像

**Files:**
- Create: `crates/agent-core/src/approval.rs`
- Create: `crates/agent-protocol/src/permission.rs`（若 permission 类型应归协议 crate，则落那里）
- Test: `crates/agent-core/tests/approval.rs`

**Interfaces:**
- Produces:
  - `PermissionRequest { input, reason, requestId, riskLevel, sessionId, options, toolCallId, toolName, turnId }`
    与 `PermissionOption { optionId, kind, name, description?, response }`；
  - `PermissionResponse { decision: Decision, reason?, modifiedInput?, permissionUpdates? }`；
  - `PermissionUpdate { type: "addRules", behavior: RuleBehavior, rules: Vec<PermissionRuleValue> }`；
  - `Ruleset { version: 1, allow: Vec<PermissionRuleValue>, deny: … }`——`value` 的 JSON 形状必须与
    实测逐字一致（`{"version":1,"allow":[…]}`）。

- [ ] **Step C1：按 §3.3 的逐字形状写类型 + 往返测试**（`serde` camelCase / snake_case 逐条核对）
- [ ] **Step C2：`Ruleset` 的「累积 addRules」纯函数 + 用例**（同 toolName+ruleContent 去重）

---

### Task D（**待裁决后才能做**）：`ApprovalStore` 的持久化

见文末「待裁决 D1」。在裁决前**不写**这块实现，也不引入任何 Rust sqlite 依赖——
先做 Task A/B/C，它们不依赖这个决定。

---

## 待裁决

**D1：`allow_project` 规则的持久化归谁写？**

| 选项 | 含义 | 代价 |
| --- | --- | --- |
| **(a) Rust 直连 sqlite** | `agent-core` 用 Rust sqlite 驱动写 `.mode/cli/db/db.sqlite` 的 `local_setting` | 与 M3 的 Node Host **同时打开同一个 DB 文件**；新增一个 Rust sqlite 依赖 |
| **(b) 经 Host 代理** | Rust 侧只发「加规则」请求，由 Host（`packages/services`）落库 | 需要一个**当前协议里没有的**方法（观察到的反向请求只有 preferences 与 permission 两条），属新增接口 |
| **(c) 只在合入 M3 Host 后接** | M2 期间 `ApprovalStore` 用 trait + 内存实现，把 sqlite 落库留到接线时 | M2 的对拍只到「同会话/同进程内规则生效」，跨会话落盘的对拍**留白并登记** |

**建议 (c)**：M2 的验收口径是**行为对拍**，而 (a)(b) 都要先定一个 M2 范围外的架构问题；
(c) 不阻塞 Task A/B/C，且把「跨会话落盘未对拍」如实登记而不是假装过了。

## 风险

| 风险 | 对冲 |
| --- | --- |
| `Edit` 的行为面比 `Read` 更宽（唯一性、`replace_all`、多行匹配） | Step B1 先探全再实现；探不到的写进 §6「已知误差与未决」，不猜 |
| 直连 sqlite 与 M3 Host 抢同一 DB | D1 未裁决前不实现持久化（选项 (c)） |
| 探针文件撞 oxlint `max-lines`（400 行） | 本次已按关注点拆成 `probe-write*.mts` 三件（support / write / rules） |
