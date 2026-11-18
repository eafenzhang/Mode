# Spec：C4 生态面接入与默认翻转（LT-1 C4）

状态：**方向已定（2026-11-19，用户「继续 C4」）**；实现前由 C4-T0 补齐黑盒观察。
关联：`docs/specs/agent-core-parity.md`（LT-1 总 spec，C4 = 生态面 + 20 任务 + 默认翻转）、
`docs/specs/host-ownership-switch.md`（§7 20 任务清单）、
`docs/specs/agent-c3-recovery-goal.md`（C3 携带项）、`docs/specs/clean-room-policy.md`。

## 1. 目标与范围

C4 把 C1–C3 交付的 Rust 会话运行时从「核心子集」扩到**产品级生态面**，并把默认运行时
翻转到 Rust：

1. **MCP**：会话内 MCP server（stdio/http）接入、工具目录与调用、OAuth 参数透传；
2. **Skills / Plugins / Workflows**：会话上下文注入（skills/plugins）、`workflows` 六方法、
   插件概览/安装/启停等会话面；
3. **附件与 v4 会话帧**：`attachment begin/chunk/commit`、`conversationAttachmentRead/Stat`、
   `v4 conversation` 订阅/帧/增量、`sessions-index`、只读 query 族；
4. **automation / provider/account 面**：按 T0 观察与既有裁决落地或保持显式 unsupported；
5. **子代理执行基础面**（从 C3 只读面升级为可驱动）；
6. **20 个真实模型任务**（`host-ownership-switch.md` §7 清单）通过；
7. **默认运行时翻转**：Rust 成为默认 Agent，Node 仅显式回退。

**不在 C4**：官方平台连接、遥测、工作流可视化 UI 重做、打包签名变化。

## 2. 生态面接入纪律

- 会话面只接入本仓库既有事实源：`mcp-sync-service`（配置）、`skills-service`、
  `plugin-host`（执行）、`session-store`（workflows 行）；不在 agent-server 里复制第二份
  配置/状态（复用 `agent-protocol` 严格镜像与 C2/C3 已验收的读面模式）。
- MCP server 子进程由会话运行时托管：stdio 挂接/工具目录刷新/失败诊断，遵循
  `docs/specs/mcp-sync-service.md` 与 T0 观察；OAuth 参数按 `session/create` 协议透传。
- 附件：走 T0 观察的 `attachment begin/chunk/commit/abort` + `conversationAttachmentRead/Stat`
  形状；落盘遵守 C1 的 `<dataRoot>` 布局。
- v4 帧：`v4/conversation/subscribe` 等只读查询与帧在 C4 接入 Rust；帧形状复用
  `agent-protocol::v4` 镜像，不发明字段。
- automation 10 个方法：按 C3-T0 裁决由 Renderer 分别调用两个 owner 的口径保持；
  provider/account 2 个网络方法保持显式 `unsupported_method`（去智谱化不变），
  除非 T0 观察到必须实现的会话内行为。

## 3. 20 任务验收（§7 清单）

- 执行前提：`MODE_HOST_INPROCESS_CHANNELS` 含 `mode-agent`；使用真实模型与可用 workspace；
- 记录：日志、关键帧、逐项通过标准（§7 表 1–20 + 补充检查）；
- 失败项：根因 + 修复记录，不以超时掩盖；
- MCP/Skills/Plugins/Workflows 相关任务（#15/#17/#18/#19）在本波具备真实执行条件。

## 4. 默认翻转

- `resolve_agent_command`：默认解析顺序改 Rust（打包 `agent-server` 优先）→ 仍保留
  `MODE_AGENT_SERVER_COMMAND` 回退 Node；健康面 `runtime=rust` 为默认值；
- Node 回退路径保留测试；`host-ownership-switch.md` §7 的 20 任务是翻转门禁；
- 翻转后 Node 不再为同一 workspace 启动（沿用 C2 的共享 DB 串行写语义）。

## 5. 验收

1. T0 契约 + 夹具（每生态面至少一条 happy path + 一条失败/边界）；
2. C4 回放 harness：新增生态面夹具全绿，C1 39/39 + C2 11/11 + C3 13/13 保持；
3. 20 任务逐项记录（日志/关键帧），失败项有根因修复；
4. 默认翻转后：两 workspace 连续启动仅 Rust 子进程；回滚开关可用；
5. 门禁全绿；未观察/未支持项登记 §C4。

## 6. Clean-room

同前：不读上游/旧实现体；只读协议镜像、specs、既有 Rust 事实源、黑盒观察；
生态面提示词/注入文案由本仓库编写。
