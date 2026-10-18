# M2 对拍覆盖率矩阵（Task 7 输入）

状态：2026-10-18 首版。**这是盘点，不是验收通过**；具体用例归属以各测试文件为准。

## 结论先说

- `packages/services/test` 共 **60** 个 `*.test.ts`，其中 **0** 个引用运行时会话面
  （`SessionEvent` / `session/send` / `agentSession` / `CommandInbox` / `tool.updated`）。
- 因此阶段计划 Task 7 的「在 services 的 ~198 个测试里挑运行时用例」**前提已过期**：
  服务包的测试以 IM bot / CUA / 插件市场 / provider 配置 / 存储路径为主。
- 运行时的行为规格实际在下面四处：
  1. `packages/desktop/tests/agent-runtime-oracle/fixtures/*.json`（oracle 录制）；
  2. 同目录的 `*.test.mts`（夹具回放、协议 schema、可重复性）；
  3. `crates/agent-core/tests/*` 与 `crates/agent-tools/tests/*`；
  4. `crates/agent-protocol/tests/*`（协议帧夹具与 progress 形状）。
- **建议**：把 Task 7 的盘点对象改成上面四项；`packages/services/test` 只保留「不适用」的
  判定记录，不再逐文件转写。

## `packages/services/test` 分类（60 个文件）

| 域 | 数量 | 判定 | 理由 |
| --- | --- | --- | --- |
| `bot-*` | 25 | 不适用 | IM bot 业务；M2 不含 bot 宿主（M3） |
| `cua-*` | 8 | 不适用 | 电脑控制独立线 |
| `dataRoot*` / `dataBaseDir*` | 5 | 不适用 | 存储路径/迁移；归 Host（M3） |
| `plugin-*` / `official-plugin-attribution` | 5 | 不适用 | 插件市场（M3） |
| `provider*` / `orcarouter*` / `remoteModelCatalog` / `retiredZhipu*` | 9 | 不适用 | provider 配置层；Rust 侧目前只有脚本化 fake（DR-T5-2） |
| `conversationMirror*` / `importedClaudeRecovery` / `nonCliAcpRetirement` / `conversation-typing-bubble` | 4 | 转不动（登记原因） | 镜像/会话存储/宿主层；不是 stdio 运行时事件面 |
| `credential-service` / `wecom-registration` / `weixin-provider-self-id` / `windowsCuaDevRootContract` | 4 | 不适用 | 凭据 / 通道注册 / CUA 根 |

<details>
<summary>逐文件清单</summary>

- **bot（25）**：`bot-attachment-cache-retention`、`bot-change-summary-kinds`、
  `bot-config-schema`、`bot-conversation-session`、`bot-dingtalk`、`bot-group-chat`、
  `bot-group-history-store`、`bot-group-prompt`、`bot-heartbeat`、`bot-message-queue`、
  `bot-outbound-delivery`、`bot-outbound-target`、`bot-remote-workspace-identity`、
  `bot-reply-granularity`、`bot-reply-segments`、`bot-reply-text-format`、
  `bot-session-binding-eligibility`、`bot-state-mutation`、`bot-streaming-edit`、
  `bot-text-media`、`bot-typing-feedback`、`bot-wecom-typing`、`bot-weixin-typing`、
  `bot-workspace-binding`、`bot-workspace-turn-gate`
- **cua（8）**：`cua-enablement`、`cua-helper-admission`、`cua-helper-stdio-relay`、
  `cua-overview-attribution`、`cua-permission-observation-gate`、`cua-plugin-distribution`、
  `cua-raw-input`、`cua-rd-media`
- **data（5）**：`dataBaseDirDefault`、`dataBaseDirEnvLock`、`dataRootInitializer`、
  `dataRootMigration`、`dataRootOwnership`
- **plugin（5）**：`plugin-description-translation`、`plugin-detail-components-state`、
  `plugin-marketplace-defaults`、`plugin-marketplace-id-compat`、`official-plugin-attribution`
- **provider（9）**：`providerBalanceParsing`、`providerBalanceProvider`、
  `providerBalanceTargetResolver`、`providerConfigMigration`、`remoteModelCatalog`、
  `retiredZhipuProviderMigration`、`orcarouter.live`、`orcarouter`、`orcarouterProviderOverlay`
- **host/转不动（4）**：`conversationMirrorService`、`importedClaudeRecovery`、
  `nonCliAcpRetirement`、`conversation-typing-bubble`
- **misc（4）**：`credential-service`、`wecom-registration`、`weixin-provider-self-id`、
  `windowsCuaDevRootContract`

</details>

## 运行时的对拍资产（真正的覆盖来源）

| 位置 | 覆盖 |
| --- | --- |
| `packages/desktop/tests/agent-runtime-oracle/fixtures/*.json` | oracle 录制的行为夹具（Read / Write / Edit / Bash 等） |
| `packages/desktop/tests/agent-runtime-oracle/*.test.mts` | 夹具三层一致、协议 schema、可重复性、数据根隔离 |
| `crates/agent-core/tests/tool_parity.rs` | 工具调用序列 + 文件快照对拍（Read/Write/Edit/Bash） |
| `crates/agent-core/tests/turn_stream.rs` | 流式 turn 的事件顺序与终态统计 |
| `crates/agent-core/tests/task_control.rs` / `task_notification.rs` | TaskOutput/TaskStop 顺序、后台完成通知 |
| `crates/agent-core/tests/provider_script.rs` | 脚本化 provider 的确定性 |
| `crates/agent-core/tests/session_turn.rs` | `turnNumber` 起点 |
| `crates/agent-tools/tests/read_only.rs` + `src/task.rs` 单测 | 工具纯函数层与任务注册表 |
| `crates/agent-protocol/tests/fixtures.rs` / `progress.rs` | 协议帧形状（含 progress） |

## CI 门禁（2026-10-18，Task 7 Step 2）

**它是什么**：对「分类是否过期」的守卫，不是行覆盖率统计。运行：

```bash
pnpm exec tsx scripts/agent-runtime-parity-matrix.ts --check
```

挂在 release CI 的 `verify` job（`Language policy metric` 之后），失败即阻断发版。

**规则**：

1. `packages/services/test/*.test.ts` 每个文件必须命中且只命中一条分类规则
   （`bot` / `cua` / `data` / `plugin` / `provider` / `host-untranslatable` / `misc`）；
   出现新文件而没有任何规则命中 → 退出 1，提示补分类（写明「已转 / 转不动 / 不适用」）。
2. 「运行时对拍资产」清单里的路径必须存在（上面那张表的路径）；文件被删/改名而清单未更新
   → 退出 1。清单只列**入口文件**，目录级资产（oracle fixtures）按目录存在性检查。
3. 输出固定摘要（各域文件数、未分类文件、缺失资产），`--json` 可输出机器可读版本。

**不做什么**：不把 services 测试逐个转成 Rust（前提已过期，见上）；不计算行覆盖率；
不检查 Rust 测试是否真的断言了某个场景——那由各测试文件自身保证。

判定函数与清单在脚本里是纯函数导出（`classifyServicesTestFile` / `buildParityReport`），
由 `packages/desktop/tests/agent-runtime-parity-matrix.test.mts` 覆盖；改分类规则时先改
本 spec，再改脚本与测试。

## 待办（Task 7 后续）

1. 若确认上述口径，更新阶段计划的 Task 7 步骤与阶段验收第 5 条（不再以
   `packages/services` 为覆盖率分母）。〔2026-10-18：Task 7 Step 1/2 已按此口径落地〕
2. 覆盖率报告进 CI 前，先定义「已转 / 转不动 / 不适用」的文件清单格式（本文件的表格即输入）。
   〔2026-10-18：格式即脚本内的分类规则与上面的 CI 门禁一节〕
3. 真实模型 20 个端到端任务（阶段验收第 3 条）仍未执行，需人参与与计费，不在自动巡检范围。
