# M2 Task 4：`Bash` 与 `CommandInbox` 串行准入 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `subagent-driven-development`（推荐）或
> `executing-plans` 逐任务实现。步骤用 `- [ ]` 复选框跟踪。
>
> 本计划是 `docs/plans/2026-10-16-tauri-agent-runtime.md` 的 **Task 4 子计划**，按那份计划
> 文末的约定在开工时产出。阶段权威仍是路线图的 M2 节；本文件只补「怎么落地」。
>
> **纪律**：`docs/specs/clean-room-policy.md`——只运行旧实现观察行为，不读
> `apps/mode-cli/**`、`packages/services/src/**`、`packages/client/src/**` 的实现体。

**Goal:** 让 Rust 侧的 `Bash` 在**行为**上与旧运行时对拍通过（输出、退出码、超时、取消），
并实现 `CommandInbox` 的串行准入语义，不用超时掩盖同步问题。

**Scope（先说清楚）：**

- 只做 `Bash`（`run_in_background`、`TaskOutput`、`TaskStop` 的完整语义属于本任务还是拆出去，
  见 DR-T4-5）。
- 不做 MCP / Skills / 子代理（M3 sidecar）。
- 不改 TS 真源（DR-07）；Rust 是镜像，用共享夹具双向断言。

---

## 已观察到的行为（2026-10-17，`probe-bash.mts`）

复现：

```bash
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-bash.mts
```

| 场景 | 结果（逐字） |
| --- | --- |
| `printf 'hello\nworld\n'` | `success=true`，`content: "hello\nworld"`（**末尾换行被去掉**） |
| `exit 7` | **`success=true`**，`content: "Exit code 7"`（非零退出不是工具错误） |
| `printf 'oops\n' >&2` | `success=true`，`content: "oops"`（**stderr 合并进 content**） |
| `pwd` | `/c/Users/…/mode-probe-ws-…`（**Git Bash / MSYS 路径**，不是 `C:\…`） |
| 不存在的命令 | `success=true`，`content: "Exit code 127\n/usr/bin/bash: line 3: …: command not found"` |
| `sleep 5` + `timeout: 1000` | `success=true`，`content: "Command timed out after 1s\n<error>Command was aborted before completion</error>"` |
| `{}`（缺 `command`） | `success=false`，`type=tool_execution_failed`，`message=Tool input failed inputSchema validation` |

声明面（`probe-write.mts surface`）：`Bash` 的 `input_schema` 为
`{command: string(必填), timeout?: number(max 600000), description?: string,
run_in_background?: boolean, dangerouslyDisableSandbox?: boolean}`，
`additionalProperties: false`。

**结论**：非零退出码与超时都**不是**工具层错误；工具错误只覆盖「入参校验失败」这类前置失败。
这条与 `Write`/`Edit` 的 `tool_execution_failed` 语义不同，不能套用。

---

## 仍未观察（Step 0 必须先做，不许猜）

阶段计划要求对拍覆盖「超时、取消、退出码」，目前只拿到退出码与超时，且有两处只有表象：

1. **取消（用户中断）**：现有 driver 只有 `send()` 与 `stop()`，没有取消原语。
   需要先黑盒探出会话面的取消方法名与事件（可能是 `session/interrupt` / `turn/cancel` 一类），
   再比较「取消」与「超时」的内容差异。
2. **超时是否真的杀掉进程（含孙进程）**：`content` 里的 aborted 文案不能证明 OS 层已清理。
   需要观察超时后 `sleep` 子进程是否存活（Windows 上按 PID/命令行查）。
3. **pty 与 pipe 两种模式的输出差异**：`pwd` 给出 MSYS 路径说明至少走了一次 shell；
   但「输出是否逐行流式」「是否伪终端」没有观察。
4. **输出截断/artifact 的阈值**：结果里有 `truncated=false`、`budgetStrategy=artifact`，
   但阈值与超限后的形状未测。
5. **`run_in_background` / `TaskOutput` / `TaskStop`**：未测。它会影响「一个 turn 何时结束」，
   必须决定本轮做还是拆出去。
6. **`CommandInbox` 的准入顺序**：一个 mock script 里串行调用看不到并发。
   需要能在一个 turn 运行中再发 `session/send`（或直接写原始帧），观察第二条输入是排队、
   拒绝还是合并；以及 key 的粒度（session / workspace / 全局）。

**Step 0 的产物**：`docs/specs/tauri-agent-runtime.md` §3.4（逐字观察）+ 扩展后的探针，
以及把这些未知逐条写成「已观察 / 未观察」。

---

## 决策记录

### DR-T4-1：shell 后端用 `portable-pty`（按阶段计划），但先确认对拍面

阶段计划指定 `portable-pty`。它引入一个新的第三方 crate，需要：

- 加进根 `Cargo.toml` 的 `[workspace.dependencies]`（版本钉死，走 `Cargo.lock`）；
- 更新 `third-party/inventory.json`（`inputs` 归一与 `copied.files` 两套哈希都要跑，
  见 `third-party` 台账的既有口径）；
- 若 `portable-pty` 的输出行为（换行、缓冲、EOF）与 oracle 不一致，**行为以 oracle 为准**，
  必要时在 `bash.rs` 里做适配层，而不是把差异写成「已知差异」蒙混过去。

**代价（若判错）**：`portable-pty` 在 Windows 上的 ConPTY 行为与 Git Bash/MSYS 不完全相同；
若 oracle 实际走的是 `bash -lc` + pipe，那么引入 pty 反而制造差异。所以 Step 0 必须先观察
「输出来源是 pty 还是 pipe」，再决定是否真的需要 pty。

### DR-T4-2：非零退出码 → `success=true` + `Exit code N` 行

已观察。Rust 侧照此实现：退出码不是错误，内容第一行是 `Exit code {code}`。
`command not found`（127）同理。

### DR-T4-3：超时 → `success=true` + 两行文案

已观察：`Command timed out after {秒}s` + `<error>Command was aborted before completion</error>`。
超时**不是** `tool_execution_failed`；取消与超时的文案若在 Step 0 观察到不同，按观察分开建模。

### DR-T4-4：输出规范化

已观察：stderr 合并、末尾换行去掉。实现里把「合并 + trim 末尾换行」做成**纯函数**并单测，
避免与 oracle 的差异散落在进程处理代码里。

### DR-T4-5：`run_in_background` 与 `TaskOutput` / `TaskStop` 的归属

阶段计划的 Task 4 大纲没有列出后台任务；但 `Bash` 的声明里有 `run_in_background`。
Step 0 观察它的返回形状后二选一：

- 若后台任务需要独立的任务注册表与跨 turn 唤醒 → **本轮只做前台**，后台登记为 Task 4b
  （或 Task 5，取决于它是否与会话流绑定）；
- 若它只是「立即返回 task id + 结束后发一条事件」→ 并入 Task 4。

**不许**在没观察前假装实现。

---

## Task 步骤

### Step 0：补齐观察面（先做，阻塞后续）

- [ ] 扩展 driver 的观测口：允许在 turn 运行中写原始帧（用于取消与并发准入），
      不改变既有 `send()` 的语义。
- [ ] 探取消：找到会话面的取消方法，记录请求/事件/最终 content。
- [ ] 探超时后的进程清理：记录 `sleep` 子进程在超时前后是否存活。
- [ ] 探输出模式：大输出（> 阈值）是否 `truncated=true`、artifact 落点与形状。
- [ ] 探 `run_in_background` 的返回与后续事件。
- [ ] 探并发 `session/send`：第二条输入在 busy/running 下的结果与 key 粒度。
- [ ] 结论写进 spec §3.4；未观察到的逐条标注。

### Step 1：spec 先行

- [ ] 在 `docs/specs/tauri-agent-runtime.md` 增补 §3.4「`Bash` 与准入」：输入 schema、
      退出码/超时/取消的逐字文案、输出规范化、CommandInbox 的键与顺序。
- [ ] 把 Step 0 的「未观察」留在 §6 的未知清单，不写成事实。

### Step 2：夹具与对拍用例

- [ ] 用 `record.mts` 录制前台场景夹具：成功、非零退出、stderr、命令不存在、超时、
      取消、入参校验失败。
- [ ] 在 `crates/agent-core/tests/bash_and_inbox.rs` 写第一层（工具调用序列）与
      第二层（文件快照，若命令产生文件）断言；输出内容逐字比较。

### Step 3：`Bash` 实现

- [ ] `crates/agent-tools/src/bash.rs`：入参校验、执行、输出合并与 trim、退出码、超时。
- [ ] 超时/取消的进程终止要**可验证**（测试里用可观察的子进程，不用 `sleep` 猜测）。
- [ ] 不做 `run_in_background`（除非 DR-T4-5 判定并入）。

### Step 4：`CommandInbox`

- [ ] `crates/agent-core/src/command_inbox.rs`：`admit(key)` 返回准入结果；
      同 key 的 busy/running 输入串行排队，顺序由观察决定。
- [ ] **禁止**用超时掩盖同步问题；用例直接断言准入顺序。

### Step 5：门禁与收口

- [ ] `node scripts/rust-cargo.ts test -p agent-core -p agent-tools` 全绿。
- [ ] `node scripts/rust-cargo.ts clippy --all-targets -- -D warnings` 零告警。
- [ ] `pnpm verify:pre-push` 通过。
- [ ] 更新阶段 ledger 的 Task 进度；本地提交（不推送）。

---

## 验收

1. 前台 `Bash` 的七类场景与 oracle 逐字一致（成功、非零退出、stderr、命令不存在、超时、取消、
   入参校验失败）；
2. 超时/取消后进程确实被终止（测试可复现，不靠时间窗口）；
3. 同 key 的并发输入按观察到的顺序串行准入；
4. 未观察到的后台任务与 pty 差异如实登记，不写成已支持。

## 风险

| 风险 | 对冲 |
| --- | --- |
| `portable-pty` 与 oracle 的输出模式不一致 | Step 0 先判定 pty vs pipe；行为以 oracle 为准 |
| 超时只杀父进程、孙进程残留 | 用可观察子进程写用例；必要时按进程组终止 |
| 取消与超时被当成同一种结果 | Step 0 分别观察，文案不同就分开建模 |
| 并发准入无观察依据 | 扩展 driver 后再实现；没有观察就不写「顺序保证」 |
| `run_in_background` 把范围撑大 | DR-T4-5 明确二选一，不默认实现 |
