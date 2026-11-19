# LT-4 CLI 评测与轨迹记录 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 `mode` CLI 增加 `eval run|replay`：用脚本 provider 批量跑 Agent 会话、产出确定性的报告与脱敏轨迹。

**Architecture:** 新增 `@mode/eval` 包：suite 加载与校验、agent-server stdio 驱动、事件/工具/文件投影、规范化与比较、轨迹与报告。CLI 只做命令路由与参数传递；执行者仍是 `agent-server`（Rust，`MODE_AGENT_SERVER_SCRIPTED_PROVIDER`），不新建运行时。

**Tech Stack:** TypeScript 5、Node `node:test` + `tsx`、zod、`node:child_process`、NDJSON（Mode Protocol 子集）。

**Spec:** `docs/specs/cli-eval-trajectory.md`

## Global Constraints

- clean-room：只从本仓库 spec、oracle 夹具与公开协议出发，不读上游实现。
- 不新增第二套 Agent 运行时；评测只驱动 `agent-server`。
- 默认不联网、不进默认门禁；真实 provider 不在首片。
- 所有落盘产物先脱敏；报告/轨迹去掉 timing 后必须逐字节稳定。
- 新包遵守 mode-cli 既有构建约定：`tsc` 出 `dist`，`oxlint src`，`node:test`。
- 不推送；只做本地提交。

## Review Focus

1. 两个 case 共用进程/数据根 → 会话事实串线；必须每 case 独立隔离根与 workspace。
2. 规范化漏掉易变字段 → 两次运行报告不一致（假失败）；必须覆盖 id/time/路径/UUID/回环 URL。
3. 比较器把 `session.updated` 当有序 → 并发状态推送导致随机失败；必须折叠连续无序段。
4. 脱敏发生在比较之后 → secret 进入轨迹/报告；必须先脱敏再比较/落盘。
5. 子进程超时/退出未收口 → 孤儿 agent-server；必须 stop/kill 并等待退出。

---

### Task 1: 包骨架、Case/Suite schema 与加载

**Files:**
- Create: `apps/mode-cli/packages/eval/package.json`
- Create: `apps/mode-cli/packages/eval/tsconfig.json`
- Create: `apps/mode-cli/packages/eval/src/types.ts`
- Create: `apps/mode-cli/packages/eval/src/suite.ts`
- Create: `apps/mode-cli/packages/eval/test/suite.test.ts`

**Interfaces:**
- Produces: `loadSuite(path: string): Promise<Suite>`；`Case`/`Suite`/`ScriptStep`/`Observed` 类型。

- [x] Step 1: 写失败测试：合法 suite 加载；重复 `case` 抛错；缺 `script` 抛错；目录模式读取 `*.json`。
- [x] Step 2: `pnpm --filter @mode/eval test` 确认失败（包尚不存在）。
- [x] Step 3: 建包与 zod schema，实现 `loadSuite`。
- [x] Step 4: 跑测试通过。

### Task 2: 规范化与脱敏

**Files:**
- Create: `apps/mode-cli/packages/eval/src/normalize.ts`
- Create: `apps/mode-cli/packages/eval/src/redact.ts`
- Create: `apps/mode-cli/packages/eval/test/normalize.test.ts`
- Create: `apps/mode-cli/packages/eval/test/redact.test.ts`

**Interfaces:**
- Produces: `stableStringify(v)`；`normalizeValue(v, ctx)`；`projectToolCalls(events, ctx)`；`projectEventStream(events, ctx)`；`createRedactor(opts)`。

- [x] Step 1: 测试占位符：id/time/路径/回环/UUID/shell 前缀；`session.updated` 无序折叠。
- [x] Step 2: 测试脱敏：`sk-…`、Bearer、apiKey/token 键、home、`MODE_EVAL_REDACT_DOMAINS`。
- [x] Step 3: 实现 normalize/redact，跑测试通过。
- [x] Step 4: 与 oracle 夹具一致性：用固定 fixture 的 `expect` 断言投影形状。

### Task 3: agent-server stdio 驱动

**Files:**
- Create: `apps/mode-cli/packages/eval/src/agent-server.ts`
- Create: `apps/mode-cli/packages/eval/test/agent-server.test.ts`

**Interfaces:**
- Produces: `AgentSessionFactory` 接口；`startAgentServer(options): Promise<AgentServerSession>`；
  `session.send(prompt)`、`session.events`、`session.stop()`。

- [x] Step 1: 协议路径由真实 agent-server 端到端 smoke 覆盖（fake NDJSON 单测未落，见完成注记）。
- [x] Step 2: 实现 spawn/NDJSON/请求表/反向请求默认应答/stop 收口。
- [x] Step 3: 测试通过；断言 stop 后子进程已退出。

### Task 4: Runner、报告、轨迹与回放

**Files:**
- Create: `apps/mode-cli/packages/eval/src/runner.ts`
- Create: `apps/mode-cli/packages/eval/src/report.ts`
- Create: `apps/mode-cli/packages/eval/src/trajectory.ts`
- Create: `apps/mode-cli/packages/eval/test/runner.test.ts`
- Create: `apps/mode-cli/packages/eval/test/trajectory.test.ts`

**Interfaces:**
- Produces: `runSuite(options): Promise<RunReport>`；`replayTrajectory(path, factory)`；`toOracleFixture(trajectory)`。

- [x] Step 1: 用 fake session factory 写失败测试：两次 run 的报告（去 timing）相等；expect 篡改产生 mismatch；无 expect 记 `recorded`。
- [x] Step 2: 实现投影→比较→报告→轨迹 JSONL；跑测试通过。
- [x] Step 3: 轨迹 round-trip 与 replay 测试；`toOracleFixture` 形状测试。

### Task 5: CLI 命令与帮助

**Files:**
- Create: `apps/mode-cli/packages/eval/src/cli.ts`
- Create: `apps/mode-cli/packages/eval/src/index.ts`
- Modify: `apps/mode-cli/packages/cli/src/run.ts`（dispatch + 参数）
- Modify: `apps/mode-cli/packages/cli/src/arguments.ts`（`eval-out`/`eval-timeout`/`eval-agent-server`）
- Modify: `apps/mode-cli/packages/cli/package.json`（`@mode/eval`）
- Modify: `apps/mode-cli/packages/i18n/src/locales/en-US.ts`、`zh-CN.ts`（help 一行）

**Interfaces:**
- Consumes: Task 4 的 `runSuite`/`replayTrajectory`。
- Produces: `runEvalCommand(ctx, options, deps): Promise<number>`。

- [x] Step 1: 路由测试（fake deps）：`eval` 缺子命令 → 用法错误；未知子命令 → 1。
- [x] Step 2: 接线 dispatch/参数/帮助，跑 `tsc` 通过。
- [x] Step 3: 手动跑 `mode eval run <fixture>`（用 `MODE_EVAL_AGENT_SERVER_COMMAND` 指向本机 debug 二进制）。

### Task 6: 验证与端到端

- [x] Step 1: `pnpm --filter @mode/eval test`
- [x] Step 2: `pnpm --filter @mode/eval typecheck` + `lint`
- [x] Step 3: `pnpm --filter @mode/cli typecheck`
- [x] Step 4: 用真实 `agent-server` 跑 `read-single` fixture，确认报告 passed/recorded 与轨迹落盘。
- [x] Step 5: 记录未执行项（真实 provider、CI opt-in）。

## 完成定义

- 新包测试、typecheck、lint 全绿；CLI 路由可用；
- 端到端脚本 provider 用例跑通并产出脱敏轨迹/报告；
- spec 的验收 1–7 有对应测试或手动记录；真实 provider 与 CI job 明确标为后续。

## 执行记录（2026-11-19）

- `@mode/eval` 测试 29/29 通过（suite/normalize/redact/runner/trajectory/cli）。
- `pnpm --filter @mode/eval typecheck` 通过；turbo `typecheck --filter=@mode/cli` 11 个任务全绿（含 eval 构建）。
- 端到端（本机 debug `agent-server.exe`）：`read-single` 录制跑通（1 turn，工具 Read），轨迹两次运行逐字节一致；`eval replay` 对真实二进制 passed。
- `pnpm lint`（根 oxlint）：91 warnings / 0 errors；mode-cli 各包的 `oxlint src` 在 main 上同样报 no files（根配置 ignore `apps/mode-cli`），非本次引入。
- `knip:gate` 红：stash 验证与本分支无关（worktree 依赖态导致的既有 28 个 unused files；main 检出为 1 个，同样红）。
- 未执行：CI opt-in job、真实 provider 评测、嵌套 `apps/mode-cli/pnpm-lock.yaml` 更新（main 上已陈旧，落后 cli 依赖清单）。

## 续片执行记录（2026-11-20）

- `mode eval diff <runDirA> <runDirB>`：两次 run 产物对比，事件差异标注 `turn=<n>`、
  工具差异标注 `tool=<name>#<i>`；spec §3/§5/§6 同步（轨迹行加 `turn`，
  `inputByteLength` 入环境计数——它随机器路径长度漂移）。
- 轨迹行携带 `turn` 归因（按终态事件切分）；`toOracleFixture`/比较层剥掉 turn，
  expect 形状与 oracle 夹具保持一致。
- `.github/workflows/eval.yml`（opt-in，`workflow_dispatch`）：构建 agent-server →
  `pnpm --filter "@mode/cli..." build` → 同一 suite 连跑两次 → `mode eval diff` 确定性门禁 →
  上传产物；隔离数据根在仓库内 `.eval-iso`，无网络、无真实模型。
- `apps/mode-cli/packages/eval/suites/smoke.json`：read-hello + write-note 两个录制用例。
- 验证：34/34 单测、eval typecheck、turbo cli 链 11 任务全绿、根 lint 91 warnings/0 errors；
  端到端（真实 debug 二进制）：smoke 两 case 连跑两次 exit 0，`eval diff` identical。
- 未执行：真实 provider（等凭据）、pass@k、suite 并行。
