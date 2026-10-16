# M2 Agent 运行时 Rust 化 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `subagent-driven-development`（推荐）或
> `executing-plans` 逐任务实现。步骤用 `- [ ]` 复选框跟踪。
>
> **本文件的分层，开工前先读**：Task 0–2 是**逐步骤可执行**的（含真实代码），
> Task 3–7 是**步骤级大纲**——它们各自的逐步骤细节在开工时另出子计划，理由见文末
> 「为什么 Task 3–7 只到步骤级」。这是 M2 的**阶段计划**，不是一次能执行完的单次计划。

**Goal:** 把会话循环、工具执行、权限引擎与双链路语义搬到 Rust，且与旧 TS 运行时
**行为对拍**通过——即「同一输入喂两个运行时，工具调用序列 / 文件快照 / 事件流三层一致」。

**Architecture:** 绞杀者的第二刀。旧 TS 运行时（`mode app-server --stdio`）在 M2 全程
**继续在现网跑**，并在测试里充当 **oracle**；Rust 侧新增 `crates/agent-protocol`（v4 帧协议镜像）、
`crates/agent-core`（会话循环 / CommandInbox / owner-lease / 权限引擎）、`crates/agent-tools`
（工具实现）。对拍套件是这一阶段的**主产物**：先有它，再有实现。

**Tech Stack:** Rust 1.99（`stable`，见根 `rust-toolchain.toml`）、tokio、portable-pty、
serde/serde_json、schemars（仅新增的 Tauri 面用）；测试侧复用 `tsx --test` + 旧运行时进程。

**Spec:** `docs/plans/2026-10-09-tauri-rewrite-roadmap.md` 的 M2 节（阶段边界与验收）；
本阶段实现 spec 于 **Task 0** 产出：`docs/specs/tauri-agent-runtime.md`。
纪律：`docs/specs/clean-room-policy.md`——`crates/agent-*/**` 全部适用。
上游依据：`docs/specs/tauri-platform-commands.md` 是 M1 已交付的**平台面**（M2 会复用它的
`platform::commands` 来弹审批 UI）。

## Global Constraints

- **clean-room**：`packages/services/src/**`、`packages/client/src/**`、`apps/mode-cli/**` 的
  **实现体禁读**。允许读：协议类型声明（`packages/shared/src/mode-protocol-v4/**` 的
  zod schema 与类型）、测试断言、`docs/specs/**`。对拍只比**行为**（进程的输入输出与事件序列），
  不打开旧实现照着写。
- **协议真源方向（DR-07）**：M2 期间**运行时协议仍以 TS 为真源**，Rust 侧是镜像，用**共享 JSON
  夹具**双向断言（见 Task 1）。真源翻转到 Rust 是 M4/M5 删掉 TS 运行时时的事——此刻翻转会让
  「对拍」退化成「自己对自己」，失去意义。
- **目标协议版本（DR-08）**：只打 **v4**（`packages/shared/src/mode-protocol-v4/**`）。
  `mode-protocol/index.ts`（v1）是渲染层的历史面，不在 M2 范围。
- **既有语义不得丢失**：owner/lease、`CommandInbox` 串行准入、`desktop-continuous` 与
  `web-remote-replayable` 双链路、`workspaceIdentity` 贯穿——每项都要有对拍用例。
- **不做的事**：插件/MCP/Skills 宿主（M3 sidecar）、UI 替换（M4）、浏览器自动化（M3 sidecar）。
- 语言政策：新增只允许 Rust / TS / HTML / CSS。夹具用 JSON（数据文件，不受语言政策约束）。
- 每个任务结束都要 `cargo clippy --all-targets -- -D warnings` 零告警 + 本机可跑的用例全绿。

### 与路线图 M2 节的口径修正（读原文前先读这里）

路线图 M2 的任务 1 写的是「read/glob/grep/**list** —— 只读工具先行」。**实际工具名不是这样**：
真源是 `packages/shared/src/tool-identity.ts` 的 `MODE_KNOWN_TOOL_NAMES`，取值为 PascalCase，
且**没有 `list` 工具**。

> **2026-10-16 修正（Ruling 17，实测）**：上面那张表本身也有错。`MODE_KNOWN_TOOL_NAMES` 是一张
> **渲染用**的名字表，不等于运行时真正注册的工具集。Task 2 的实测结果是：旧运行时在 stdio 会话面
> 上**只注册了 `Read`**，调用 `Glob` / `Grep` 返回 `tool_not_found`（已录进夹具
> `packages/desktop/tests/agent-runtime-oracle/fixtures/read-absent-search-tools.json`）。
> **M2 的只读切片因此 = `Read`**，不实现 `Glob`/`Grep`——M2 的验收是「与旧运行时行为对拍」，
> 实现一个运行时没有的工具等于**没有 oracle 可比**。下表的 `Glob`/`Grep` 两行按此作废；
> 其余行（`Bash`/`Write`/`Edit`/`ApplyPatch`/订阅型）**尚未实测**，Task 3 起开工前必须先跑
> 一次工具面观察，不要照抄本表。

| 路线图写法 | 实际工具名 | M2 归属 |
| --- | --- | --- |
| read | `Read` | Task 2（**唯一实测存在的只读工具**） |
| glob | ~~`Glob`~~ | **作废**：运行时未注册，调用得 `tool_not_found` | 
| grep | ~~`Grep`~~ | **作废**：同上 |
| list | *不存在* | 不新造工具 |
| （未提） | `Bash` | Task 4（shell 与 pty）——**未实测，开工前先观察** |
| （未提） | `Write` / `Edit` / `ApplyPatch` | Task 3——**未实测，开工前先观察** |
| （未提） | `WebFetch` / `WebSearch` | **M3**（要网络策略与 sidecar 的取回面） |
| （未提） | `Task` / `Agent` / `Skill` / `js` / `mcp__node_repl__*` | **M3**（子代理与 node-repl 宿主都在 sidecar） |
| （未提） | `TodoRead` / `TodoWrite` / `GoalRead` / `AskUserQuestion` / `SendMessage` 等 | Task 5/6（要会话状态与交互面）——**未实测** |

`MODE_KNOWN_TOOL_NAMES` 里还有一档 `ModeToolFamily`（`file-read | file-write | shell | search |
todo | ask-user-question | agent | skill | goal | session-context | message | task-control |
node-repl | workflow`），权限引擎的规则表按**family** 分档比按工具名分档更稳，Task 6 用它。

---

### Task 0: 对拍 oracle 与夹具框架（本阶段的地基）

**为什么它是 Task 0**：M2 的验收口径是「行为对拍通过」。没有 oracle 就先写实现，等于凭印象重写，
既违反 clean-room（要靠读旧实现才知道该做什么），也没法验收。所以先把「起一个旧运行时、
喂输入、收三层输出、和 Rust 侧逐层比」这件事做成可复用的测试框架。

**Files:**
- Create: `docs/specs/tauri-agent-runtime.md`（阶段 spec，含 Task 0 的 oracle 契约）
- Create: `crates/agent-core/Cargo.toml`、`crates/agent-core/src/lib.rs`（先只放 `Runtime` trait 与事件枚举占位）
- Create: `packages/desktop/tests/agent-runtime-oracle/`（oracle 驱动，TS 侧）
  - `packages/desktop/tests/agent-runtime-oracle/driver.mts`（起旧运行时、喂 prompt、收事件）
  - `packages/desktop/tests/agent-runtime-oracle/fixtures/*.json`（共享夹具，见 Step 4）
- Test: `packages/desktop/tests/agent-runtime-oracle/driver.test.mts`

**Interfaces:**
- Consumes: 无（起点）。
- Produces:
  - `startOracleRuntime(options: { dataRoot: string; cwd: string }): OracleSession`，
    其中 `OracleSession = { send(prompt: string): Promise<void>; events: AsyncIterable<OracleEvent>;
    stop(): Promise<void> }`。
  - `OracleEvent = { type: string; payload: unknown }`——`type` 取自 v4 的 `ModeSessionEventType`。
  - 夹具格式：`{ "case": "<名字>", "prompt": "<输入>", "expect": { "toolCalls": [...],
    "files": {...}, "events": [...] } }`。

- [ ] **Step 1: 写阶段 spec 的 oracle 一节**（不是代码，但要先落地，因为后面所有任务都引用它）

`docs/specs/tauri-agent-runtime.md` 至少含四节：**oracle 契约**（怎么起、喂什么、收什么）、
**对拍三层口径**（工具调用序列 / 文件快照 / 事件流各自的比较规则）、**不平等的合法差异**
（哪些字段允许不同：时间戳、id、进程内路径、事件到达顺序中标注为 unordered 的部分）、
**M2 不做的面**（上表里归 M3 的工具）。第三点尤其重要：不给「合法差异」列白名单，
对拍会因时间戳逐条报错而被关掉。

- [ ] **Step 2: 写失败的 driver 测试**

```ts
// packages/desktop/tests/agent-runtime-oracle/driver.test.mts
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { startOracleRuntime } from "./driver.mjs";

test("oracle 能起来、能吐事件、能干净退出", async () => {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-oracle-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-oracle-ws-"));
  await writeFile(join(cwd, "hello.txt"), "hello\n", "utf8");

  const session = await startOracleRuntime({ dataRoot, cwd });
  try {
    await session.send("读一下 hello.txt 然后回一句话作为回答：它写了什么？");
    const seen: string[] = [];
    for await (const event of session.events) {
      seen.push(event.type);
      if (event.type === "turn.completed" || event.type === "turn.failed") break;
    }
    // 这一条断言只钉「驱动真的收得到会话事件」，不钉模型输出——
    // 后续用例才拿它做行为对拍。
    assert.ok(seen.includes("turn.started"), `没收到 turn.started，实收：${seen.join(",")}`);
    assert.ok(
      seen.includes("turn.completed") || seen.includes("turn.failed"),
      `turn 没有终态，实收：${seen.join(",")}`,
    );
  } finally {
    await session.stop();
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
});
```

- [ ] **Step 3: 跑测试确认失败**

Run: `pnpm exec tsx --test packages/desktop/tests/agent-runtime-oracle/driver.test.mts`
Expected: FAIL——`Cannot find module './driver.mjs'`。

- [ ] **Step 4: 实现 driver**

要点（每一条都是踩过的坑，照写）：

1. **启动方式**：`mode app-server --stdio`（口径见 `scripts/mode-distribution/runner.mjs` 的
   `MODE_AGENT_SERVER_ARGS_JSON`）。本地开发用 `pnpm --dir apps/mode-cli exec tsx packages/cli/src/main.ts
   app-server --stdio`，不要依赖已构建的 `dist/mode.cjs`。
2. **数据根隔离**：`MODE_DATA_ROOT`（或既有等价变量，见 `docs/specs/mode-data-root.md`）指到
   Step 2 里 `mkdtemp` 出来的目录——对拍**绝不能碰用户真实的 `~/.mode`**。
3. **模型**：对拍要可重复，必须用**录制/回放**而不是打真实 API。先在 driver 里留出
   `MODE_ORACLE_PROVIDER=mock` 的开关位，具体 mock provider 由 Step 4b 落地。
4. **事件解析**：逐行读 stdout，按 v4 帧解析（`packages/shared/src/mode-protocol-v4/transport.ts`
   的 `TopicFrame` / `V4_METHODS`）；解析失败的行进 `stderr` 汇总，不要静默丢。
5. **退出**：`stop()` 先发优雅停止，超时（建议 5s）再 kill；杀不干净会让后续用例抢不到端口/锁。

- [ ] **Step 4b: mock provider（对拍可重复性的前提）**

没有这一步，对拍会被模型的不确定性污染：同一个 prompt 两次跑出不同工具序列，用例就会随机红。

**这一步的前置查证已在 2026-10-16 做完，结论是「没有可复用的面，要新建」**（查证范围与结果
写下来，省得下一个人再查一遍）：

| 线索 | 查询结果 |
| --- | --- |
| `packages/desktop/mock-cdn/` | 只有 `components/` 与 `releases/`，是插件市场的 mock CDN，不是模型侧 |
| `config/provider/mode-builtin.json` | 只有 `schemaVersion`/`revision`/`config`，是内置 provider 清单 |
| `packages/provider/src/**`、`packages/provider-node/src/**` | 无 `test` / `mock` / `fake` 的 provider id |
| 全局搜 `mockProvider` / `MODE_MOCK` / `cassette` / `recordReplay` / `fixtureProvider` | 无命中 |
| 全局搜 `MODE_E2E` / `fakeProvider` / `stubProvider` | 只命中 `run-production-build.mjs`（与模型无关） |

所以走 **(b)：新建一个本地 mock provider**——按 prompt 哈希返回固定的 tool_calls / 文本序列。
落点建议 `packages/provider-node/src/`（那里放 provider 的具体实现），入口用一个
`MODE_ORACLE_PROVIDER` 之类的显式开关，**默认关闭**，绝不能在不设开关时被开发者的日常运行选到。

**验收**：同一个 prompt 连跑两次，工具调用序列与应用事件流（除时间戳/id）逐字相同。

- [ ] **Step 5: 三层对拍比较器 + 第一批夹具**

Create: `packages/desktop/tests/agent-runtime-oracle/compare.mts`，导出三个纯函数：

```ts
// 三层各自的比较规则（白名单差异在 spec 里，这里只实现）
export function compareToolCalls(actual: ToolCall[], expected: ToolCall[]): Diff[];
export function compareFileSnapshot(actual: Record<string, string>, expected: Record<string, string>): Diff[];
export function compareEventStream(actual: OracleEvent[], expected: OracleEvent[]): Diff[];
```

夹具至少 3 条，覆盖：只读一次成功、读不存在的文件（错误路径）、同一次 turn 里连续两次工具调用。
三条夹具都要**先用旧运行时跑一遍录下来**（就是 oracle 的用法），不要手写期望值——
手写期望值等于把「我以为的行为」当规格，那正是 clean-room 要避免的。

- [ ] **Step 6: 跑通并提交**

Run: `pnpm exec tsx --test packages/desktop/tests/agent-runtime-oracle/*.test.mts`
Expected: PASS（driver 用例 + 3 条夹具的 oracle 侧录制用例）。

```bash
git add crates/agent-core docs/specs/tauri-agent-runtime.md packages/desktop/tests/agent-runtime-oracle Cargo.toml
git commit -m "test(agent-runtime): M2 Task 0——对拍 oracle、mock provider 与三层比较器"
```

---

### Task 1: `crates/agent-protocol`——v4 帧协议的 Rust 镜像

**Files:**
- Create: `crates/agent-protocol/{Cargo.toml,src/lib.rs}`
- Create: `crates/agent-protocol/tests/fixtures.rs`
- Create: `packages/desktop/tests/agent-runtime-oracle/protocol-fixtures.test.mts`（TS 侧同夹具断言）
- Create: `packages/shared/test/fixtures/agent-protocol/*.json`（**共享夹具，两端都读它**）

**Interfaces:**
- Consumes: Task 0 的夹具目录约定。
- Produces: `agent_protocol::Frame`（`TopicFrame` 的 Rust 对应）、
  `agent_protocol::session_event::SessionEvent`（`ModeSessionEvent` 的对应）、
  `agent_protocol::command::{CommandEnvelope, CommandResult, CommandAck}`。

- [ ] **Step 1: 写两端的同夹具断言（先失败）**

共享夹具是这一步的核心：同一份 JSON，TS 侧用 zod 解析（`mode-protocol-v4` 的真实 schema），
Rust 侧用 serde 解析（镜像类型）。**任何一端加字段而另一端没跟上，必有一侧红。**

Rust 侧：

```rust
// crates/agent-protocol/tests/fixtures.rs
use std::path::PathBuf;

#[test]
fn every_shared_fixture_parses_into_the_mirror_types() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/shared/test/fixtures/agent-protocol");
    let mut count = 0;
    for entry in std::fs::read_dir(&dir).expect("夹具目录不存在") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        // 解析失败要带文件名，否则夹具一多就找不到是谁坏了。
        let value: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("{} 不是合法 JSON：{e}", path.display()));
        serde_json::from_value::<agent_protocol::session_event::SessionEvent>(
            value.get("sessionEvent").cloned().expect("夹具缺 sessionEvent"),
        )
        .unwrap_or_else(|e| panic!("{} 的 sessionEvent 解不进镜像类型：{e}", path.display()));
        count += 1;
    }
    assert!(count > 0, "夹具目录是空的，这条用例等于没跑");
}
```

TS 侧（`packages/desktop/tests/agent-runtime-oracle/protocol-fixtures.test.mts`）读**同一目录**，
用 `mode-protocol-v4` 的真实 schema 解析同一个 `sessionEvent` 字段。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p agent-protocol --test fixtures`
Expected: FAIL——crate 不存在 / `agent_protocol::session_event` 未定义。

- [ ] **Step 3: 实现镜像类型**

只镜像 M2 要用到的面（YAGNI）：会话事件的判别联合、命令封套与结果、快照与增量的**外壳**
（`ConversationSnapshot` 的内层字段按 Task 5 需要逐步补齐）。**不要一次把 v4 全量搬过来**——
v4 有几十个类型，先搬「对拍要用到的」，用例驱动的补法比一次性翻译可靠，也更符合 clean-room
（翻译整套 schema 已经接近「按上游思路复现」）。

命名与字段严格对齐 TS：`#[serde(rename_all = "camelCase")]`；判别联合用
`#[serde(tag = "type", rename_all = "snake_case")]` 时**必须逐条核对 v4 的线上取值**——
`OAuthCallback` → `o-auth-callback` 这类 serde 命名转换的坑在 M1 已经踩过一次
（见 `crates/protocol/src/lib.rs` 的 `DeepLinkEventKind` 注释）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p agent-protocol && pnpm exec tsx --test packages/desktop/tests/agent-runtime-oracle/protocol-fixtures.test.mts`
Expected: 两端全绿。

- [ ] **Step 5: 提交**

```bash
git add crates/agent-protocol packages/shared/test/fixtures/agent-protocol packages/desktop/tests/agent-runtime-oracle/protocol-fixtures.test.mts Cargo.toml
git commit -m "feat(agent-protocol): M2 Task 1——v4 帧协议的 Rust 镜像与同夹具双向断言"
```

---

### Task 2: 只读工具（**`Read`**——`Glob`/`Grep` 运行时未注册，见上方口径修正）

**Files:**
- Create: `crates/agent-tools/{Cargo.toml,src/lib.rs,src/read.rs}`
- Modify: `crates/agent-core/src/lib.rs`（接上工具注册表与最小会话循环）
- Test: `crates/agent-tools/tests/read_only.rs`、`crates/agent-core/tests/read_only_parity.rs`

**Interfaces:**
- Consumes: `agent_protocol::{session_event, command}`。
- Produces:
  - `agent_tools::ToolSpec { name: &'static str, family: ModeToolFamily, schema: serde_json::Value,
    read_only: bool }` 与 `agent_tools::registry() -> Vec<ToolSpec>`；
  - `agent_core::tool::invoke(spec: &ToolSpec, args: &serde_json::Value, ctx: &ToolContext)
    -> Result<ToolOutcome, ToolError>`。

- [ ] **Step 1: 写对拍用例（先失败）**

`crates/agent-core/tests/read_only_parity.rs`：读 Task 0 的三条夹具，跑 Rust 侧工具，
用 `compareToolCalls` / `compareFileSnapshot` 的 Rust 对应物比较。

**Rust 侧怎么跑 TS 的 oracle**：对拍套件整体由 TS 侧发起更简单（TS 起旧运行时、起 Rust 侧
的可执行文件、比较），Rust 侧的用例只测「给定 args → 输出」的纯函数层。**别做双向驱动**——
两个进程互相起会带来复杂的生命周期问题，而收益为零。

- [ ] **Step 2: 跑测试确认失败**；**Step 3: 实现 `Read`**（只读、无副作用）
      〔2026-10-16 按 Ruling 17 修正：原写「实现三个工具」，但 `Glob`/`Grep` 运行时未注册，
      不实现；见上方口径修正表〕

`Read` 的关键语义（用对拍确认，不要凭印象）：行号前缀格式、截断上限与截断提示文案、
二进制文件与超长行的处理、`offset`/`limit` 的边界。
**另外三类是 Task 2 实测才发现必须覆盖的**（原表没列，都是行为而不是猜测）：
① 同一 path+range 的**重复读取去重**（返回 "Wasted call …" 桩，会话级、跨 turn 生效，键是
**解析后的路径 + range**，错误不去重）；② 超预算时的**部分视图**与它的行数预算；
③ 未注册工具的**同形答复**（`tool_not_found`），即「调用一个运行时没有的工具」本身也是一条
要对拍的行为。

- [ ] **Step 4: 跑测试确认通过**；**Step 5: 提交**

```bash
git commit -m "feat(agent-tools): M2 Task 2——只读工具 Read/Glob/Grep 与对拍用例"
```

---

### Task 3: 文件写与 diff 呈现（`Write` / `Edit` / `ApplyPatch`）+ 审批三态

**Files:**
- Create: `crates/agent-tools/src/write.rs`、`src/edit.rs`、`src/apply_patch.rs`
- Create: `crates/agent-core/src/approval.rs`
- Modify: `crates/platform/src/commands/*`（审批 UI 走 M1 的命令面）
- Test: `crates/agent-core/tests/write_approval.rs`

**Interfaces:**
- Produces: `agent_core::approval::{Decision, ApprovalStore}`，
  `Decision = AllowOnce | AllowAlways | ProjectAlways | Deny`；
  **数据文件与旧版同格式**（路线图要求 M4 迁移零成本）——格式以 oracle 观察为准，不猜。

- [ ] **Step 1: 三态持久化的格式对拍**

先让旧运行时跑出一次 `allow-always`，观察它写出的文件内容与路径，**把它当格式规格**，
再实现 `ApprovalStore`。这一步的产物是一份「格式取自 oracle」的说明，写进 spec。

- [ ] **Step 2–5: TDD 实现三个写工具 + 审批**（大纲；细步骤见开工时另出的子计划）

`Edit` 的「字符串替换必须唯一」、`ApplyPatch` 的多文件与失败回滚、`Write` 覆盖已存在文件时的
审批分级——全部以夹具对拍为准。diff 呈现（渲染层看到的 `ToolUpdated` 事件形状）用
`compareEventStream` 比。

- [ ] **Step 6: 提交**

---

### Task 4: shell 与 pty（`Bash`）+ `CommandInbox` 串行准入

**Files:**
- Create: `crates/agent-tools/src/bash.rs`（portable-pty）
- Create: `crates/agent-core/src/command_inbox.rs`
- Test: `crates/agent-core/tests/bash_and_inbox.rs`

**Interfaces:**
- Produces: `agent_core::command_inbox::CommandInbox::admit(key: CommandKey) -> Admission`
  ——同一 key 的 busy/running 输入**串行准入**（`AGENTS.md` 的既有语义，不得用超时掩盖）。

- [ ] **Step 1–6: 对拍含超时、取消、退出码语义**（大纲；细步骤见开工时另出的子计划）

必须覆盖的判据：超时后进程是否真的被杀（含孙进程）、取消（用户中断）与超时的区分、
退出码与信号的非零呈现、pty 与 pipe 两种模式下输出缓冲的差异、
**并发两条命令在同一 key 下的准入顺序**。

---

### Task 5: 会话流与重放（双链路各自验证）

**Files:**
- Create: `crates/agent-core/src/session.rs`、`src/stream.rs`、`src/replay.rs`
- Modify: `crates/agent-protocol/src/lib.rs`（补齐快照/增量需要的类型）
- Test: `crates/agent-core/tests/dual_linkage.rs`

**Interfaces:**
- Produces: `agent_core::runtime::Runtime::run(prompt) -> impl Stream<Item = SessionEvent>`。

- [ ] **Step 1: 两条链路各写一套 E2E**

- `desktop-continuous`：**不落重放**，断线即丢（迟到的事件不再补发）。
- `web-remote-replayable`：快照 + 队列**重放**，重连后从快照恢复并补发队列里的事件。

判据来自 `docs/specs/remote-workspace-conversation-mirror.md` 与
`docs/specs/lan-remote-computer-use.md` 的「状态所有者 / 事件顺序」两节
（已核实这两节存在）。**改 stream / snapshot / queue 时必须双跑**——这是路线图
「风险与对冲」里点名的对冲手段。

- [ ] **Step 2–5: TDD 实现**（大纲）；**Step 6: 提交**

---

### Task 6: 权限引擎（规则表驱动）

**Files:**
- Create: `crates/agent-core/src/permission.rs`
- Test: `crates/agent-core/tests/permission_engine.rs`

**Interfaces:**
- Produces: `agent_core::permission::Engine::evaluate(tool: &ToolSpec, args: &Value) -> Decision`。

- [ ] **Step 1–5: 按 `ModeToolFamily` 分档实现 allow / deny / ask 规则表**（大纲）

数据文件与旧版同格式（同 Task 3 的做法：格式取自 oracle）。工具级确认（Tool-level confirmation）
与 `AskUserQuestion` 的交互面复用 M1 的命令面。

---

### Task 7: 对拍矩阵收口

**Files:**
- Create: `scripts/agent-runtime-parity-matrix.ts`（语言政策：新增禁入 `.mjs`）
- Modify: `docs/specs/tauri-agent-runtime.md`（覆盖率报告与「转不动的用例」清单）

- [ ] **Step 1: 盘点 `packages/services/test/*.test.ts`（59 个文件）里与运行时相关的用例**

逐个判定三档：**已转**（有 Rust 侧等价用例）/ **转不动**（记录原因进 spec）/ **不适用**
（与运行时无关，如 bot-*、cua-*、plugin-*）。产出一张表，作为覆盖率报告的输入。

- [ ] **Step 2: 把覆盖率报告接进 CI**（新 job 或挂进现有 `verify`）

- [ ] **Step 3: 提交**

---

## 阶段验收

1. 对拍套件全绿（三层一致：工具调用序列 / 文件快照 / 事件流）；
2. 双链路 E2E 各自通过（改 stream/snapshot/queue 时双跑）；
3. 用真实模型跑通 **20 个端到端任务**（改代码 / 跑测试 / 浏览器）无行为回归；
4. `cargo clippy -D warnings` 零告警；`cargo test` 全绿；
5. 覆盖率报告：`packages/services` 里与运行时相关的用例有明确的「已转 / 转不动（带原因）」归属。

## 风险

| 风险 | 对冲 |
| --- | --- |
| **对拍依赖模型输出**，同一 prompt 两次跑出不同序列 → 用例随机红 | Task 0 Step 4b 先把 mock/录制 provider 做掉，并加「同 prompt 连跑两次逐字相同」的自检 |
| 行为对拍测不出隐性语义（双链路、owner/lease） | Task 5 双链路各自 E2E + 阶段验收第 3 项的 20 个真实任务盲跑 |
| 一次性翻译整套 v4 schema ≈ 按上游思路复现，踩 clean-room 红线 | Task 1 只搬对拍要用到的类型，用例驱动补齐；阶段 spec 明确记录这条边界 |
| 旧运行时与新运行时抢同一 `~/.mode` 数据根 | Task 0 强制 `mkdtemp` 隔离；对拍用例里加断言「数据根不在用户目录下」 |
| 工期（4–6 个月）| 每个 Task 结束都是可停点；Task 2 结束即「只读工具在 Rust 侧可用」，可独立交付 |
| `CommandInbox` 用超时掩盖同步问题（`AGENTS.md` 明令禁止） | Task 4 的用例直接断言准入**顺序**，而不是「最终都跑完了」 |

## 2026-10-16 说明

本轮（第二批执行）只**产出本计划**，未开工写实现。理由有两条，都是硬约束：

1. 路线图的 Global Constraints 写明「每个阶段（M0–M5）在开工前必须按 `writing-plans` 技能
   单独产出一份细粒度子计划」——本文件就是 M2 的那一份；
2. 本机只能验 Windows，而 M2 的核心是**对拍**，需要在能起旧运行时的环境里跑真实模型与
   mock provider；先按 Task 0 把 oracle 与夹具框架立起来，再谈实现。

**开工后第一件事**：Task 0 Step 4b 的 mock provider（它是整阶段可重复性的前提）。
前置查证已在 2026-10-16 完成并写进 Task 0——仓库里**没有**现成的 mock/录制面可复用，
按结论新建即可，不用再查。

---

## 为什么 Task 3–7 只到步骤级

两层原因，都不是偷懒：

1. **它们的详细规格只能从 oracle 观察里得到**。`Read` 的截断提示文案、`Glob` 的忽略目录、
   `Edit` 的「替换必须唯一」报错文本、审批三态的数据文件格式——这些**不能靠猜**：
   写下来的话，等于把「我以为的行为」当规格，而 M2 的全部意义恰恰是「以旧运行时的实际行为
   为规格」（clean-room §黑盒对拍）。Task 0 的 oracle 立起来之后，这些细节才有真源可抄。
   在这之前写满代码块，产出的是一份需要全部重写的计划。
2. **规模**。M2 是 4–6 个月的工作量，一次写成的「完整代码级计划」会大到没人能执行，
   而且必然有过半内容在第一个 Task 完成后作废。`writing-plans` 的 Scope Check 对这种
   「一个计划覆盖多个独立子系统」的情形给的建议就是**拆成多个计划**。

所以本文件同时承担两个角色：**M2 的阶段计划**（边界、决策记录 DR-07…DR-10、任务切分、
验收口径、风险对冲），以及 **Task 0–2 的细粒度子计划**。Task 3–7 开工时各出一份：

| 任务 | 开工时应产出的子计划 |
| --- | --- |
| Task 3 文件写与 diff + 审批三态 | `docs/plans/2026-XX-XX-agent-write-approval.md` |
| Task 4 shell 与 pty + CommandInbox | `docs/plans/2026-XX-XX-agent-shell-inbox.md` |
| Task 5 会话流与双链路重放 | `docs/plans/2026-XX-XX-agent-dual-linkage.md` |
| Task 6 权限引擎 | `docs/plans/2026-XX-XX-agent-permission-engine.md` |
| Task 7 对拍矩阵收口 | `docs/plans/2026-XX-XX-agent-parity-matrix.md` |

这与 M1 的实际做法一致：`2026-10-15-tauri-skeleton.md` 也只在一开始就写满了任务骨架，
细节是随执行逐步补齐的（见该文件的「执行记录」）。
