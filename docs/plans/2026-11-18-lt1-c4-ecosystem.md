# LT-1 C4：生态面接入与默认翻转 Implementation Plan

> **For agentic workers:** 本文件是 C4 检查点的长任务计划。每个 Task 开工前按
> `writing-plans` 补齐该 Task 的逐步骤细节；步骤用 `- [ ]` 跟踪。
>
> 前置：C1–C3 已合并 main（C3 merge `843e38aff`）。C4 在
> `feat/lt1-c4-ecosystem` 上开发。

**Goal:** 把 MCP/Skills/Plugins/Workflows/附件/v4 帧/automation/subagent 执行接入 Rust
会话运行时，通过 20 个真实模型任务，并把默认 Agent 运行时翻转到 Rust。

**Architecture:** 会话面全部走 `agent-protocol` 严格镜像 + 既有 Rust 事实源
（`mcp-sync-service`/`skills-service`/`plugin-host`/`session-store`/`subagents-service`），
由 per-session worker 串行承载；事件/DB 走单一 EventPublisher。

**Tech Stack:** 同 C1–C3；MCP stdio/http 客户端、`plugin-host` sidecar、`skills-service`。

**Spec:** `docs/specs/agent-c4-ecosystem.md`；总 spec `docs/specs/agent-core-parity.md`；
20 任务清单 `docs/specs/host-ownership-switch.md` §7。

## Global Constraints

- 同 C1–C3：clean-room、观察优先、单一所有者、终态恰好一次、回放不回退
  （C1 39/39 + C2 11/11 + C3 13/13）、门禁全绿、不推送。
- 20 任务为翻转硬门禁；失败项有根因修复记录。
- 去智谱化不回退：provider/account 2 方法保持显式 unsupported，除非 T0 观察到会话内
  必须行为并经用户裁决。

## Task 总览

| ID  | 任务                                           | 规模 | 依赖      | 主要落点                                                                                                            |
| --- | ---------------------------------------------- | ---- | --------- | ------------------------------------------------------------------------------------------------------------------- |
| T0  | C4 黑盒契约捕获                                | L    | 无        | `docs/specs/agent-c4-contract.md`、probes/fixtures（MCP/Skills/Plugins/Workflows/附件/v4/automation/subagent 执行） |
| T1  | MCP 会话面                                     | L    | T0        | `agent-server/src/mcp.rs`、`mcp-sync-service` 接线                                                                  |
| T2  | Skills 上下文 + Plugins 面                     | L    | T0        | `skills-service` 注入、`methods/plugins.rs`                                                                         |
| T3  | Workflows 面与持久化                           | L    | T0        | `methods/workflows.rs`、`session-store` workflow 行                                                                 |
| T4  | 附件 + v4 帧/query                             | L    | T0        | `methods/attachments.rs`、`v4` 订阅/帧/query                                                                        |
| T5  | Subagent 执行基础面 + automation/provider 裁决 | M    | T0、T1–T4 | `subagent` 执行最小闭环                                                                                             |
| T6  | 20 任务验收 + 默认翻转                         | XL   | T1–T5     | 20 任务清单、`resolve_agent_command` 默认、回滚开关                                                                 |
| T7  | 对拍、smoke 与文档                             | M    | T6        | replay 扩展、§C4、smoke                                                                                             |

**波次**：Wave 0 = T0；Wave 1 = T1/T2/T4（不同文件可串行）；Wave 2 = T3/T5；Wave 3 = T6；
Wave 4 = T7。T6 含真实模型任务（需用户环境/凭据），失败修复循环不计入 5 轮上限
（每项独立根因修复）。

---

### T0: C4 黑盒契约捕获

**Files:**

- Create: `docs/specs/agent-c4-contract.md`
- Create: `packages/desktop/tests/agent-runtime-oracle/fixtures/c4/*.json`
- Create: `packages/desktop/tests/agent-runtime-oracle/probe-c4-*.mts`

**观察项**：

1. **MCP**：`session/create` 带 stdio MCP server → 工具目录/调用/失败诊断；http+oauth 形状。
2. **Skills**：skill 注入上下文形状、skill 引用解析、skill 触发。
3. **Plugins**：list/overview/describe/setEnabled/install/marketplace 增删、操作进度通知。
4. **Workflows**：六方法 + run 事件/actor/node/report/artifacts。
5. **附件**：begin/chunk/commit/abort/read/previewSource/Stat 全事务形状 + DB 行。
6. **v4 帧**：`v4/conversation/subscribe` 帧/增量/快照、`sessions-index`、只读 query 族。
7. **automation/provider/account**：automation 方法在 stdio 面的真实形状（若可达）；
   provider/account 2 方法的会话内行为。
8. **subagent 执行**：Task/Agent 工具触发子代理 turn 的形状（T0 升级到执行观察）。
9. **20 任务清单核对**：§7 每项标注「本波可实现/依赖项」。

**验收**：契约文档 + 夹具（每面 happy/失败/边界）机器录制；隔离断言；C1–C3 夹具/探针不破坏。

---

### T1: MCP 会话面

**Files:** `agent-server/src/mcp.rs`（新）、`methods/create.rs`、`mcp-sync-service` 接线、tests
**验收**：stdio/http MCP 工具目录、调用、失败诊断、OAuth 参数透传；夹具形状一致。

### T2: Skills 上下文 + Plugins 面

**Files:** `agent-server/src/skills.rs`、`methods/plugins.rs`、`skills-service` 接线、tests
**验收**：skill 注入/引用/触发；plugins 读/写/市场/进度通知；夹具形状一致。

### T3: Workflows 面与持久化

**Files:** `agent-server/src/methods/workflows.rs`、`session-store` workflow 行、tests
**验收**：六方法 + run 事件/actor/node/report/artifacts；形状一致。

### T4: 附件 + v4 帧/query

**Files:** `agent-server/src/methods/attachments.rs`、`v4.rs`、tests
**验收**：附件全事务 + DB 行；v4 订阅/帧/增量/快照/query 族；形状一致。

### T5: Subagent 执行基础面 + automation/provider 裁决

**Files:** `agent-core` subagent runtime（child turn 最小闭环）、`methods` automation 裁决、tests
**验收**：Task/Agent 工具触发 child turn（最小闭环：事件/DB/结果消费）；automation/provider
按 T0 裁决落地或保持 unsupported（登记）。

### T6: 20 任务验收 + 默认翻转

**Files:** `.plan/accept-c4/` 记录、`resolve_agent_command` 默认、健康面、回滚测试
**验收**：§7 的 20 任务逐项通过/修复；默认 Rust、回滚开关测试；无 Node 并启。

### T7: 对拍、smoke 与文档

**Files:** replay 扩展、`tauri-agent-runtime.md` §C4、parity 勾选、§9 验收记录
**验收**：C4 夹具回放全绿 + C1–C3 保持；smoke 真实端点（opt-in）；差异表与代码一致。

## 执行顺序

```text
T0 → T1 → T2 → T3 → T4 → T5 → T6 → T7
```

T1–T4 都改 methods/runtime，串行；T6 需要用户环境（真实模型凭据）；T7 最后。

## 完成定义

- 全 Task 勾选；C4 spec §5 验收记录；20 任务逐项记录；
- 默认翻转 + 回滚测试；C1–C3 回放保持；C4 夹具回放全绿；
- 门禁真实通过；未观察/未支持项登记 §C4。
