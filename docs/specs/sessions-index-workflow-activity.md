# Spec：sessions-index 的 `workflowActivity` 派生（LT-14 续段）

状态：**派生器已落（2026-11-23）**——`agent_protocol::sessions_index_workflow_activity`，
14 条契约测试。**数据源仍未接**：所有 Rust 投影路径的 `workflow_runs` 都是 `None`。

## 1. 为什么派生器可以独立落地

`workflowActivity` 与 `hasBackgroundWork` 是同一处硬编码的两个字段，但修复难度不同：

| 字段 | 数据源 | 状态 |
| --- | --- | --- |
| `hasBackgroundWork` | `TaskRegistry`（进程内可达） | ✅ 已接（`ae3f91b6f`） |
| `workflowActivity` | `WorkflowRunsState`（所有 Rust 投影路径都是 `None`） | 🟡 派生器已落，数据源未接 |

派生器是**纯函数**（同一 snapshot 的 `workflowRuns` + `backgroundWorks` → 有界摘要），
所以可以先落并逐条验语义；接数据源是另一件事（要从 `dwf_run`/`dwf_actor`/`dwf_node`
读出 run 状态，或复用 `handle_workflow_runs` 的查询）。

## 2. 为什么不能直接订阅 `workflowRuns`

侧栏要在会话标题下画「Workflow 图标 + 迷你轨道灯 + 当前 phase 名」，但 `workflowRuns`
是 conversation 权威投影，按 run 进度**高频变化**。为一个小圆点订阅整棵树不划算，
所以派生一份**有界（≤4 run）、只装侧栏真正读的字段**的摘要。与 TS 同口径。

## 3. 三条容易写错的地方

### 3.1 站点灯要区分「控制流到达」与「还有节点在跑」

控制流只记得最后一个标记（`currentPhase`），但并行的 A 在 B 之后仍可能在干活。所以
除了 `currentPhase`，还要看「出生在这一站且此刻还在跑」的节点
（`node.phase ∈ {executing, repairing, nudged}`）。

**只按 `currentPhase` 判断的后果**：并行阶段的前一站灯会提前熄灭，用户看到的是
「这条轨道已经走完了」而实际还有 agent 在跑。

### 3.2 `dispatched` / `waiting` 不算在跑

- `dispatched`：会话就绪、首个请求尚未准入的**短暂相位**；
- `waiting`：在等槽位或在退避。

真正发出去了由 `executing` 说。这两态若算作 running，站点灯会闪。

### 3.3 裁表之后下标空间跟着变窄

`alongside` 是**声明表里的下标**。裁到 `maxPhases` 之后，指向被裁掉站点的引用要一起
丢掉，剩下空的就不建键——否则 UI 会拿到越界下标去画线。

## 4. `stopped` 与 `errored` 的区别（一处专门的坑）

| run 状态 | 当前站 | 已进入的站 | 没进过的站 |
| --- | --- | --- | --- |
| `completed` | done | done | pending |
| `errored` | **failed** | done | pending |
| `stopped` | **pending**（空心灯） | done | pending |

`stopped` 的当前站必须显式判 `is_current` **先于** `was_entered`：它大概率也在
`entered` 里（进过才有当前），漏掉这一支会把「停下的站」画成已完成——与「空心灯」
的语义正好相反。这条是写测试时抓出来的。

## 5. 排序与截断

在跑的 run（`pending`/`running`）按 `runs[]` 序（= 启动序）在前；其后是已结束的，
按后台工作的 `endedAt` 倒序、无 `endedAt` 时按 `runs[]` 倒序（越晚建的越新）。
总数裁到 `SESSION_WORKFLOW_ACTIVITY_MAX_RUNS`（4）。

会话一个 run 都没有 → `None`（键整个缺席，旧 CLI 因此什么都不变）。

## 6. 只有 `kind === "workflow"` 的后台工作才算

`name` 与 `startedAt` 取自对应的后台工作记录（`work_id === run_id`）。`bash` /
`subagent` 的记录绝不能匹配上——否则 run 会显示成一条 bash 命令的名字。

## 7. 验收

`cargo test -p agent-protocol --lib sessions_index_workflow_activity`（14 条）：

- 无 run → `None`；
- live run 的 current=running、已进入=done、未进入=pending；
- 并行节点仍在跑 → 该站保持 running（不只按 currentPhase）；
- `dispatched`/`waiting`/`queued`/`settled` 都不算在跑；
- errored 只把当前站标 failed，未进入的站不标；
- stopped 的当前站是 pending（**这条抓出了上面 §4 的 bug**）；
- 声明表优先于已进入表，并携带 `alongside`；
- `alongside` 的越界与自身引用被丢弃；
- live 在前、settled 按 `endedAt` 倒序；
- 非 workflow 的后台工作被忽略；
- 总数裁到 4；
- 长站名只在上界处前缀匹配（「计划」不误认「计划修复」）；
- `agentsWorking` 只数 running 的 actor；
- JSON 形状与线上 schema 一致（缺席键不出现）。

## 8. 未做（这是下一步）

**接数据源**：把 `WorkflowRunsState` 从 `dwf_run`/`dwf_actor`/`dwf_node` 读出来填进
投影。现状是四条 Rust 投影路径（`v4_projection.rs` 两处、`server.rs`、`v4_frames.rs`）
的 `workflow_runs` 全为 `None`，所以派生器目前拿到的是空输入。

产品路径上 sessions-index 由 `apps/mode-cli` 的
`sessions-index-projection.ts` 产出（那里已正确实现），所以**用户侧暂无功能缺失**；
缺的是 Rust 侧自持投影时的等价能力。
