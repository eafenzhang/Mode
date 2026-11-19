# Spec：C4 Agent Server 黑盒契约（MCP / Skills / Plugins / Workflows / 附件与 v4 帧 / automation / subagent 执行）

状态：**C4-T0 捕获完成（2026-11-19）**。本文只记录 oracle（现网 Node `mode.cjs` 源码入口）
的黑盒观察结果，作为 C4 Rust `agent-server` / `agent-core` 的对拍基线；不是产品设计稿。
实现取舍以 `docs/specs/agent-c4-ecosystem.md` 与 C4 plan 为准，本文只回答「线上到底长什么样」。

关联：`docs/specs/agent-c4-ecosystem.md`、`docs/plans/2026-11-18-lt1-c4-ecosystem.md`、
`docs/specs/agent-c3-contract.md`、`docs/specs/agent-c2-contract.md`、
`docs/specs/agent-server-c1-contract.md`、`docs/specs/host-ownership-switch.md`（§6 决策 2/4、§7 20 任务）、
`docs/specs/mcp-sync-service.md`、`docs/specs/clean-room-policy.md`。

## 0. 来源、纪律与复现

| 项            | 值                                                                                                               |
| ------------- | ---------------------------------------------------------------------------------------------------------------- |
| oracle 入口   | `node node_modules/tsx/dist/cli.mjs apps/mode-cli/packages/cli/src/main.ts app-server --stdio`                   |
| 驱动          | `packages/desktop/tests/agent-runtime-oracle/driver.mts` 的 `startOracleRuntime`                                 |
| mock provider | `127.0.0.1:0` 上的 `anthropic-messages` 端点（`mock-provider.mts`；C4 复用 C3 的 `failure`/`delayMs`/函数剧本）  |
| 隔离根        | `mkdtemp` 目录；同时注入 `MODE_DATA_BASE_DIR`、`MODE_HOME`、`HOME`、`USERPROFILE`                                |
| 会话参数      | `oracle-mock / mock-1`，`reasoningLevel:"disabled"`，`thoughtLevel:"disabled"`，`titleGenerationEnabled:false`   |
| 生态面参数    | `session/create` 额外合并 `mcpServers` / `dynamicWorkflowEnabled`（C4 对 driver 的**只增**改动，不传时路径不变） |

复现命令（从仓库根目录执行）：

```bash
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c4-mcp.mts
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c4-skills.mts
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c4-plugins.mts
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c4-workflows.mts
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c4-attachments-v4.mts
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c4-automation.mts
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c4-subagent-exec.mts
pnpm exec oxfmt packages/desktop/tests/agent-runtime-oracle/fixtures/c4/
```

- 所有 `expect` 由脚本从本次 oracle 运行的事件/应答/DB 行/文件投影后写入，不含手写期望值。
- **可复现性**：2026-11-19 本机把上述 7 个探针各连续跑两遍，`fixtures/c4/` 下 14 个夹具
  逐字节相同（含 30 秒 MCP 超时与插件 CLI 安装路径）。
- 运行相关值在 `probe-c3-support.mts` / `probe-c4-support.mts` 折叠成稳定 token：
  `<id>`、`<session-id>`、`<cwd>`、`<data-root>`、`<tmp>`、`<time>`、`<pid>`、`<count>`、
  `<log-epoch>`、`<subscription>`、`<sessions-index-subscription>`、`<sessions-index-log-epoch>`、
  `<marketplace-root>`、`<range-start>`、`<range-end>`、`<cursor>`、`<headers>`、`<retry-delay>`。
- 目标/子会话身份别名：`target-1`、`child-1`（同一夹具内稳定）。
- 隔离断言（每个夹具的 `expect.isolation`）：`dataRootManifest:true`、
  `cliLayoutSentinel:true`、`invalidStdoutLines:0`；`mockRequestPaths` 只出现 `/v1/messages`。
- `probe-c4-support.mts` 的 `extras` 约定：**键是要折叠的原始值，值是稳定 token**。
  T0 期间修正了 attachments/workflows 两处把键写成字段名的用法（原实现会把字面量
  `logEpoch`/`runId` 替换成原始值）。
- clean-room：本文没有读取 `apps/mode-cli/**`、`packages/services/**`、`packages/client/**`
  的实现体；只运行 oracle、读它写出的帧/行/文件。

### 0.1 夹具清单

| 夹具                                  | 观察面                                                       |
| ------------------------------------- | ------------------------------------------------------------ |
| `mcp-stdio-happy.json`                | stdio MCP 连接/工具目录/调用/子进程/contextUsage             |
| `mcp-stdio-failure.json`              | stdio 启动失败/非 JSON 输出/会话可用性                       |
| `mcp-http-oauth-shape.json`           | http+oauth 参数形状/loopback 404/诊断                        |
| `skills-catalog-injection.json`       | skill 发现位置/两种 authority/目录条目/首请求注入            |
| `skills-trigger.json`                 | Skill 工具调用事件流/结果展开进下一轮请求                    |
| `skills-reference-resolution.json`    | `plugins/resolveSuggestedReference` ready/unavailable/-32602 |
| `plugins-read-face.json`              | bundled 条目/overview/describe/referenceCatalog              |
| `plugins-lifecycle.json`              | CLI 安装后的 list/描述/启停/市场增删/插件 MCP/进度通知       |
| `workflows-saved-lifecycle.json`      | Skill 门禁/SaveWorkflow 落盘/六方法/move/delete              |
| `workflows-invalid-files.json`        | 非法工作流文件归类与失败形状                                 |
| `workflows-run.json`                  | `v4/command startSavedWorkflow`/run 查询/dwf\_\* DB 行       |
| `attachments-v4-frames.json`          | 附件事务/授权错误/v4 订阅帧/只读 query/DB 与落盘             |
| `automation-stdio-surface.json`       | automation 5 方法与调用后会话可用性                          |
| `provider-account-stdio-surface.json` | provider 2 方法（隔离根内 oracle-mock）                      |
| `subagent-execute-foreground.json`    | Agent 工具定义/child 请求/child DB/结果与 usage              |

## 1. MCP（`mcp-stdio-happy` / `mcp-stdio-failure` / `mcp-http-oauth-shape`）

### 1.1 stdio happy path

`session/create` 接受 `mcpServers: [{name, command, args, env}]`（stdio），会话照常创建。
`mcp/list` 的参数为 `{workspace, mcpServers, mode}`：

- `mode:"status"`（未连接时）：`{statuses:{<server>:{status:"disconnected",transport:"stdio",
toolCount:"<count>",updatedAt:"<time>"}}}`；
- `mode:"connect"`：同一 server 变为 `status:"connected"`、`transport:"stdio"`、
  `toolCount:"<count>"`、`protocolEra:"legacy"`；
- 连接后再次 `status`/`connect` 都返回 connected（无重复启动副作用）。

不传 `mcpServers` 时 statuses 为空（workspace 配置面没有配置 server）；`mcp/list` 的
server 清单来自**参数**，不是 workspace 配置。

工具目录命名：`mcp__<serverName>__<toolName>`。观察到的目录（模型请求 `tools`）：

```text
mcp__probe-mcp__echo
mcp__probe-mcp__fail
mcp__probe-mcp__slow
mcp__node_repl__js          （内置 browser-use 插件 MCP，始终存在）
```

调用 `echo`（输入 `{text:"roundtrip"}`）的事件序列：

| 事件                     | 观察载荷要点                                                                                                                                                                                                                                           |
| ------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `tool.updated scheduled` | `toolName:"mcp__probe-mcp__echo"`、`dependencies:[]`、`parallelGroupIndex:0`、`canRunParallel:true`、`schedule`、`inputOmitted:true`、`inputRef:"model_stream"`                                                                                        |
| `tool.updated started`   | `readOnly:true`、`sideEffectScope:"session"`、`startedAt`                                                                                                                                                                                              |
| `tool.updated result`    | `result.content:"echo:roundtrip"`、`display:{kind:"mcp_tool",serverName:"probe-mcp",toolName:"echo",description}`、`success:true`、`truncated:false`、`budgetStrategy:"truncate"`、`originalBytes/returnedBytes:14`、`perf:{totalMs,permissionWaitMs}` |
| `tool.updated batch`     | `successCount:1`、`errorCount:0`、`toolCallIds`                                                                                                                                                                                                        |

`result` 事件载荷**没有** `toolName`（只有 `toolCallId`），调用者靠 `display.toolName`
或内容判定工具。`fail`（MCP `isError`）仍走成功信封：
`success:true`、`content:"MCP tool returned an error:\nprobe-mcp deliberate failure"`、
`budgetStrategy:"truncate"`、字节数 56。没有观察到 `tool.updated error` 分支。

每个 turn 的 `session.updated` 用量书签 `{messageCount,toolCount,iteration,...}` 会随流式
合并时机多一条/少一条（本机观察到 1 vs 2 条），不属于 MCP 契约；夹具只保留
`turn.started` / `turn.completed` / `session.titleUpdated` / 带 `type` 的 `session.updated`。

MCP 子进程：`process/childProcesses` 返回 `{processes:[{pid,serverName,mcpSource}]}`；
自定义 server 的 `mcpSource:"custom"`，内置 node-repl 为 `mcpSource:"builtin"` + `pluginName`。
同一 server 可能出现两行（启动器 + 实际 server 进程）。

`contextUsageBreakdown` 的来源段（顺序固定）：
`system_prompt, meta_user_context, skills, system_tool_schemas, mcp_tool_schemas, messages`；
接 MCP 时 `mcp_tool_schemas` 段存在。`process/mcpTelemetry` 通知 4 条（happy）/2 条（failure）。

### 1.2 stdio 失败诊断

两种失败（`mcp/list` `mode:"connect"`）：

| server          | 终态                                     | `error`                                                       | `failureKind`          |
| --------------- | ---------------------------------------- | ------------------------------------------------------------- | ---------------------- |
| `probe-crash`   | `status:"failed"`、`toolCount:"<count>"` | `Connection closed`                                           | `process_start_failed` |
| `probe-garbage` | `status:"failed"`、`toolCount:"<count>"` | `MCP server probe-garbage connection timed out after 30000ms` | `connection_timeout`   |

- 未连接时 `status:"disconnected"`；会话仍正常创建，普通 turn 可完成。
- 失败 server 不进入工具目录（`declaredMcpTools` 只剩内置 `mcp__node_repl__js`）。
- `process/childProcesses` 只有内置 node-repl 一行。
- `contextUsageBreakdown` 仍包含 `mcp_tool_schemas` 段（段存在，值为空/自有统计）。
- 两个失败的 server 都记录 `updatedAt`；失败诊断是 `mcp/list` 应答的一部分，不是异常帧。

### 1.3 http + oauth 参数形状

`session/create` 接受两种 oauth 形状（只录参数，不做真实 OAuth）：

```json
{ "name": "probe-http-oauth", "url": "http://127.0.0.1:<port>/mcp",
  "oauth": { "grantType": "client_credentials", ... } }
{ "name": "probe-http-authcode", "url": "http://127.0.0.1:<port>/mcp-authcode",
  "oauth": { "grantType": "authorization_code", ... } }
```

本地 loopback 假端点 404 时：

- `mcp/list` `connect` 两个 server 都是 `status:"failed"`、`transport:"http"`、
  `failureKind:"network_unreachable"`、`error:"Error POSTing to endpoint: {\"error\":\"not_found\"}"`；
- loopback 收到 3 轮 POST：`/mcp` 与 `/mcp-authcode` 各 3 次，`bodyBytes` 依次为
  270、153、270（初始化/协商/重试形状），没有真实 OAuth 令牌交换；
- 没有子进程（http transport 不拉进程），也没有模型请求。

### 1.4 MCP 未观察到

- 真实 OAuth 授权码/客户端凭证流程、刷新令牌、401 后重新认证；
- sse transport（只观察 stdio 与 http 两种）；
- 工具清单运行时变更通知（`tools/list_changed`）、resources/prompts 能力；
- MCP 工具权限询问（`permissionWaitMs` 恒为 0 的本地 fixture；未触发真实权限弹窗）；
- 多 server 同名冲突、单 server 失败对其他 server 的影响（只观察了两个独立失败 server）；
- 会话重启后的 MCP 重连/工具目录刷新。

### 1.5 对 T1 的直接约束

- 目录命名必须是 `mcp__<server>__<tool>`；插件 MCP 是 `mcp__plugin_<marketplace>_<plugin>__<tool>`
  （见 §3.2 的 `mcp__plugin_fixture_fixture__echo`）。
- `mcp/list` 的 `mode:"status"|"connect"` 语义、`protocolEra:"legacy"`、失败三分类
  （`process_start_failed` / `connection_timeout` / `network_unreachable`）按夹具实现。
- 工具结果信封：`success/display/perf/truncated/originalBytes/returnedBytes/budgetStrategy`
  与 `tool.updated result` 无 `toolName` 的形状一致；MCP `isError` 不进 `error` 事件。
- 子进程登记进 `process/childProcesses`，内置 server 带 `pluginName`。

## 2. Skills（`skills-catalog-injection` / `skills-trigger` / `skills-reference-resolution`）

### 2.1 发现位置与目录形状

隔离 workspace 同时放 `.mode/skills/probe-workspace-skill/SKILL.md` 与
`.claude/skills/probe-claude-skill/SKILL.md`：

- `.mode/skills/**` 被发现；`.claude/skills/**` **没有**进入目录（黑盒对照）。
- `skills/referenceCatalog` 的 `authority:"workspace"` 与带 `sessionId` 的
  `authority:"session"` 返回同一份条目集合（本次 3 条：内置 browser-use 两个 skill +
  workspace skill）；会话 authority 只是把 plugin/workspace 合并视角固定在当前会话。
- 条目字段：`id,name,description,path,scope,enabled`；插件来源额外带 `pluginName`。
  `scope` 取值观察到 `plugin` / `workspace`。
- 未知 `sessionId`：`-32004`，`message:"Session is not active: sess_missing"`。

`plugins/referenceCatalog` 的插件条目（3 条）：
`node-repl-host@mode-plugins-official`、`browser-use@mode-plugins-official`、
`computer-use@mode-plugins-official`。

### 2.2 首个模型请求的注入形状

带 MCP server 的会话，首个模型请求：

- `system` 3 段：41 / 1218 / 5467 chars；
- `mentionsProbeSkill:false`——首个请求的 system 段**没有**出现 probe skill 名称；
- 没有合成 user skills 提醒（`reminders:[]`）；
- 最后一条 user 是 currentDate system-reminder。

即：**目录可见 ≠ 首请求注入**。T2 的注入实现以本夹具为准；「skills reminder 出现在
首请求」没有被本次观察证实，不要按旧假设写死。

### 2.3 Skill 工具触发

模型调用 `Skill({skill:"probe-workspace-skill"})` 的事件序列：

- `model.streaming`：`tool_input_start/delta/end` → `tool_call`（`input` 为完整参数）；
- `tool.updated scheduled`：`canRunParallel:true`、`schedule`、`inputOmitted:true`；
- `tool.updated started`：`readOnly:true`、`sideEffectScope:"session"`；
- `tool.updated result`：

```json
{
  "skillMetadata": { "source": "mode" },
  "result": {
    "success": true,
    "content": "<skill_content name=\"probe-workspace-skill\">\n# Skill: probe-workspace-skill\nWorkspace skill body for the C4 probe.\nBase directory for this skill: <cwd>\\.mode\\skills\\probe-workspace-skill\nRelative paths in this skill are relative to this base directory.\n</skill_content>",
    "perf": { "totalMs": "<time>" },
    "truncated": false,
    "originalBytes": 332,
    "returnedBytes": 332,
    "budgetStrategy": "truncate"
  }
}
```

- `tool.updated batch`：`successCount:1`、`errorCount:0`；
- 下一轮模型请求的 `tool_result` part 携带上述 `content`（`toolResultContainsSkillBody:true`）；
  整个 turn 共 2 次模型请求。

`budgetStrategy` 是 `"truncate"`（Skill 与 MCP 工具），不是 Agent 结果的 `"artifact"`。

### 2.4 引用解析（`plugins/resolveSuggestedReference`）

| 输入                                              | 应答                                                                                                                                                                                               |
| ------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------- |
| `stableId:"browser-use"`（裸名）                  | `status:"unavailable"`、`diagnostics:[{code:"plugin_suggested_reference_untrusted_source",message:"推荐插件不是受信任的官方 mode-plugins-official 来源",severity:"error",pluginId:"browser-use"}]` |
| `stableId:"browser-use@mode-plugins-official"`    | `status:"ready"`、`marketplace:"mode-plugins-official"`、`pluginName:"browser-use"`、`sourceTrust:"official"`、`diagnostics:[]`                                                                    |
| `stableId:"mimosa@mode-plugins-official"`         | `status:"unavailable"`、`diagnostics:[{code:"marketplace_refresh_failed",message:"Marketplace not found: mode-plugins-official",severity:"error",pluginId:"mimosa@mode-plugins-official"}]`        |
| `stableId:"nonexistent"`                          | 同裸名：`plugin_suggested_reference_untrusted_source`                                                                                                                                              |
| 缺 `stableId/operationId/clientMode/deliveryKind` | `-32602`，`data.name:"ZodError"`，`message` 逐项列出 4 个字段（含 `expected one of "desktop-continuous"                                                                                            | "web-remote-replayable"`） |

### 2.5 Skills 未观察到

- `.claude/skills` 的任何发现条件（当前隔离根下不发现；不推广到其他目录）；
- skill 启用/禁用、删除、嵌套 skill、skill 目录冲突；
- 没有 Skill 工具时的注入差异；
- SKILL.md 缺失/非法 frontmatter 的失败形状；
- skills 注入与 MCP/plugins 同时存在时的顺序与去重。

### 2.6 对 T2 的直接约束

- 目录来源必须是 `.mode/skills/**`（`scope:"workspace"`）+ 内置插件 skill
  （`scope:"plugin"`、`pluginName`）；不要加 `.claude` 发现，除非用户改需求。
- 会话 authority 只做合并视角；未知/非活动 session 用 `-32004 Session is not active: <id>`。
- Skill 工具结果正文必须逐字带 `<skill_content name="…">`、`# Skill: …`、body、
  `Base directory for this skill: <path>`、相对路径说明；`budgetStrategy:"truncate"`。
- 注入进下一轮请求的是 `tool_result` part（role=user），不是新的 system 段。

## 3. Plugins（`plugins-read-face` / `plugins-lifecycle`）

### 3.1 读面（无 marketplace 的隔离根）

`plugins/list`（workspace）：

- 3 个 bundled 官方插件：`node-repl-host`（0.6.0）、`browser-use`（0.5.1）、
  `computer-use`；`enabled:true`、`source:"official"`、`marketplace:"mode-plugins-official"`；
- 条目字段：`id,name,version,enabled,source,marketplace,skillCount,skillRootCount,
commandRootCount,components,declaredMcpServerNames,mcpServerNames,rootSource,enabledSource`；
- `components` 按 `command → skill → mcp` 排列，skill 项带 `name/description`；
- `diagnostics:[]`。

`plugins/overview`：

- `marketplaces:[]`（隔离根没有自建市场）；
- `availablePlugins` 为官方目录（`cloudbase-skills`、`mimosa`、…），条目带
  `id,name,marketplace,description,version,installed:false,listing{descriptionI18n,displayName,
displayNameI18n,category,author},sourceUnavailable:true`；
- 目录来自 bundled 快照，不需要网络。

`plugins/describe`：

- 未知插件/市场：**成功应答** `{components:[],diagnostics:[{code:"plugin_marketplace_invalid",
message:"Marketplace not found: nonexistent-marketplace",severity:"error"}]}`；
- bundled `browser-use@mode-plugins-official`：`components`（2 个 skill）+
  `metadata:{author:"Mode",version:"0.5.1"}`。

`plugins/referenceCatalog` 的条目名是 `pluginId`：
`node-repl-host@mode-plugins-official`、`browser-use@mode-plugins-official`、
`computer-use@mode-plugins-official`；带 `sessionId` 时 `authority:"session"`。

### 3.2 生命周期（CLI 安装后会话面）

CLI（隔离根）先 `plugins marketplace add <local> --scope user`，再
`plugins install fixture@fixture-marketplace --scope user`；输出逐字：

```text
Added marketplace fixture-marketplace (1 plugins)
Installed plugin fixture@fixture-marketplace (1.0.0) [enabled]
```

本地 fixture 插件（`.mode-plugin/plugin.json` + `commands/hello.md` + `skills/fixture-skill/`

- `.mcp.json`）安装后：

* `plugins/list` 的 fixture 条目：`declaredMcpServerNames:["fixture"]`、
  `mcpServerNames:["plugin:fixture:fixture"]`、`skillCount:1`、`commandRootCount:1`、
  `components` 含 command/skill/mcp 三类；
* `plugins/describe` 返回完整组件与 `declaredMcpServerNames/mcpServerNames`；
* `setEnabled(false)`：`plugin.enabled:false`，但响应里 `skillCount:1`、
  `commandRootCount:1`、`mcpServerNames:["plugin:fixture:fixture"]`；
* `setEnabled(true)`：`enabled:true`，响应里 `skillCount:0`、`commandRootCount:0`、
  `mcpServerNames:[]`（**响应字段的已知不对称**；随后的 `plugins/list` 才回到完整值）。
  实现 T2 时不要把 enable 响应当作目录事实源。
* `plugins/operationProgress` 通知在 disable/enable/update/add/remove/install/uninstall/
  reinstall 全部窗口为 **0 条**（`count:0`）；进度通知不是这些操作的必需产物。
* `marketplace/update`（按名字刷新）返回 `{marketplaces:[…],diagnostics:[]}`；
  `marketplace/add` 重复添加同一目录仍成功并返回该 market；
  `marketplace/remove` 返回 `{diagnostics:[]}`（不返回被删 market）；
* `install`：`{dependencyClosure:["fixture@fixture-marketplace"],installedPlugins:[{id,name,
marketplace,version,enabled:true,scope:"user",installPath,installedAt}],diagnostics:[]}`；
* `uninstall`：`{removedPlugin:{…,enabled:false},diagnostics:[]}`；
* `install`（reinstall）与首次 install 同形。

插件自带 MCP 进入会话工具目录并完成一次调用：

- 工具名 `mcp__plugin_fixture_fixture__echo`（`plugin_<marketplace>_<plugin>__<tool>`）；
- `tool.updated scheduled`：`display:{kind:"mcp_tool",serverName:"plugin:fixture:fixture",
toolName:"echo",description}`、`canRunParallel:false`（插件 MCP 与自定义 MCP 的
  `canRunParallel` 不同：自定义为 true）；
- 调用结果与 §1.1 同信封。

安装路径：`<data-root>\.mode\cli\plugins\cache\<marketplace>\<plugin>\<version>`。

### 3.3 Plugins 未观察到

- 真实网络 marketplace（只观察本地 directory source）；
- 插件 hook 执行、hooks trust、managed settings；
- 插件配置 `configure/resetConfig/validate/restoreBuiltin/cancelOperation` 的形状
  （探针只调用 list/overview/describe/setEnabled/marketplace/install/uninstall）；
- 操作进度通知的真实载荷（本机全部窗口 0 条）；
- 插件卸载后工具目录/子进程的即时回收时序。

### 3.4 对 T2 的直接约束

- 读面字段必须包含 `id/name/version/enabled/source/marketplace/skillCount/skillRootCount/
commandRootCount/components/declaredMcpServerNames/mcpServerNames`；
  `components` 顺序 `command → skill → mcp`。
- 未知市场的 `describe` 是成功 + diagnostics，不是错误帧。
- 插件 MCP 工具命名 `mcp__plugin_<marketplace>_<plugin>__<tool>`，serverName 用
  `plugin:<marketplace>:<plugin>`；`canRunParallel:false`。
- `operationProgress` 不强制产生；enable 响应字段的不对称按夹具记录，不作为事实源。

## 4. Workflows（`workflows-saved-lifecycle` / `workflows-invalid-files` / `workflows-run`）

### 4.1 `dynamicWorkflowEnabled` 与 Skill 门禁

`session/create {dynamicWorkflowEnabled:true}` 后，模型可直接调用
`CreateWorkflow/SaveWorkflow/StartWorkflow`。未加载 `dynamic-workflows` skill 时：

- `CreateWorkflow` 先产生 `tool.updated scheduled`；
- 然后 `tool.updated error`：`errorType:"tool_execution_failed"`，`errorMessage` 逐字：

```text
CreateWorkflow needs the `dynamic-workflows` skill loaded in this session before it accepts a script. Call the Skill tool with skill "dynamic-workflows" first — it carries the facade declarations the script is checked against, the authoring rules and this tool's full contract — then call CreateWorkflow again. Nothing was started.
```

- 再 `tool.updated batch`；没有 turn 失败。

### 4.2 保存产物与六方法

`SaveWorkflow` 落盘（project scope）逐字节：

```text
/* mode-workflow
description: probe workflow saved by tool
whenToUse: use when the C4 probe runs the saved workflow
*/
return { ok: true };
```

- 文件格式是 `/* mode-workflow` **块注释**元数据，不是 YAML `---` frontmatter；
- project 路径 `<cwd>\.mode\workflows\<name>.dwf.ts`；global 路径
  `<data-root>\.mode\workflows\<name>.dwf.ts`。

`workflows/list`（参数 `{workspace}`）：

```json
{
  "workflows": [
    {
      "name": "probe-wf",
      "description": "…",
      "whenToUse": "…",
      "scope": "project",
      "path": "<cwd>\\.mode\\workflows\\probe-wf.dwf.ts"
    }
  ],
  "invalid": [],
  "dir": "<cwd>\\.mode\\workflows"
}
```

（条目带 `args` 时原样出现在 list 项。）

`workflows/get`：`{ok:true,name,path,scope,meta:{description,whenToUse,args?},script}`。
`workflows/updateMeta`：`{ok:true,path}`；随后 get 的 `meta` 反映新 description/whenToUse/args。
`workflows/move`：`{ok:true,from,to}`（global → project）；之后 list 中 scope/path 更新。
`workflows/delete`：`{ok:true,path}`；删除不存在：`{ok:false,reason:"not_found"}`；
非法名字：`{ok:false,reason:"invalid_name",detail:"workflow names may only contain letters,
digits, '.', '-' and '_', and must be 1-64 characters"}`。

### 4.3 非法文件

`.mode/workflows/` 下放三种手写文件：

| 文件                        | `workflows/list.invalid` 归类                                                                            |
| --------------------------- | -------------------------------------------------------------------------------------------------------- |
| 无 frontmatter 的 `.dwf.ts` | `{path,reason:"missing_frontmatter: file does not start with the \`/\* mode-workflow\` metadata block"}` |
| 缺 description 的 `.dwf.ts` | 同上（YAML `---` 也被判为 missing_frontmatter）                                                          |
| 扩展名不是 `.dwf.ts`        | **完全忽略**（不进 workflows 也不进 invalid）                                                            |

`workflows/get` 对非法文件：`{ok:false,reason:"parse_error",detail:"file does not start with
the \`/\* mode-workflow\` metadata block"}`。

### 4.4 运行与查询

`v4/command` `{type:"startSavedWorkflow", payload:{name,scope}}`：

- 成功（ACK）：

```json
{
  "commandId": "cmd-c4-wf-run-ok",
  "status": "accepted",
  "revisionAtDecision": 21,
  "result": { "type": "startSavedWorkflow", "runId": "dwfrun-<id>", "toolCallId": "<id>" },
  "memoryEnabled": false
}
```

- 名字不存在：`status:"failed"`、`reasonCode:"fault.command.savedWorkflowStartRejected.not_found"`、
  `message:"No project workflow named 'missing-wf'."`（没有 result）。

`workflows/runs`（`{workspace,limit}`）：

```json
{
  "runs": [
    {
      "runId": "dwfrun-<id>",
      "name": "probe-run",
      "status": "completed",
      "createdAt": "<time>",
      "updatedAt": "<time>",
      "spentTokens": 0,
      "parentSessionId": "<session-id>",
      "toolCallId": "<id>",
      "cwd": "<cwd>"
    }
  ]
}
```

`v4/conversation/workflowRuns`：

```json
{
  "runs": [
    {
      "runId": "dwfrun-<id>",
      "toolCallId": "<id>",
      "label": "probe-run",
      "updatedAt": "<time>",
      "status": "completed",
      "resumable": false
    }
  ]
}
```

观察到的空形状：`workflowRunEvents {events:[],hasMore:false}`、
`workflowRunArtifacts {artifacts:[]}`、`workflowRunWorkspace {nodes:[]}`。
`workflowRunNodeResult` 的 schema 参数是 `siteId` + `ordinal`（不是 `nodeId`）；
传 `nodeId` 得到 `-32603` ZodError（`Unrecognized key:"nodeId"` + 缺 `siteId/ordinal`）。
成功的 node result 形状未观察到。

`dwf_*` DB 行（run 完成后）：

- `dwf_run`：`parent_session_id,cwd,name,script_text,script_hash,args_json,tool_call_id,
resumed_from,caps_max_concurrency,spent_tokens,status,result_json,failure_json,time_created,
time_updated`；
- `dwf_event` 3 行，`sequence 0/1/2`：`run-started`（`payload.caps.maxConcurrency:10`）→
  `run-launched`（`inputId,toolCallId:"launch-…",parentSessionId,scriptPath`）→
  `run-settled`（`status:"completed"`）；
- `dwf_actor` / `dwf_node` / `workflow_definition` / `workflow_event` / `workflow_run`
  在该最小脚本下都是 0 行（走的是 `dwf_*` 新表，不是 `workflow_*` 旧表）。

### 4.5 Workflows 未观察到

- 真正执行 node/actor 的 run（本次脚本立即返回，`dwf_node` 与 artifacts 为空）；
- `workflowRunEvents` 非空分页、artifact 数据读取、node result 成功形状；
- run 失败/取消/恢复（`resumable:false` 只观察到完成态）；
- `SaveWorkflow` 的 global 覆盖、同名冲突、args 校验；
- hooks/权限对工作流脚本执行的影响。

### 4.6 对 T3 的直接约束

- 文件格式必须是 `/* mode-workflow` 块注释 + 脚本；不要用 YAML。
- 六方法与 `move` 的 scope/path 语义、error `reason` 文案按夹具。
- `startSavedWorkflow` 的 ACK 必须带 `revisionAtDecision` 与 `memoryEnabled`；
  not_found 用 `fault.command.savedWorkflowStartRejected.not_found`。
- run 级事实源是新 `dwf_run/dwf_event` 表；`workflow_*` 旧表在该路径下为空。

## 5. 附件与 v4 帧（`attachments-v4-frames`）

### 5.1 附件事务

`v4/attachment/begin` → `{uploadId,state:"staging",nextChunkIndex:0}`；
`v4/attachment/chunk` → `{uploadId,chunkIndex,nextChunkIndex}`（每 chunk 递增）；
`v4/attachment/commit` → `{ref:"mode-artifact://<session>/tool-result-<id>"}`。
text/plain 与 1×1 PNG 的应答形状完全相同（二进制不改变信封）。
`abort` 返回 `{}`；abort 后再 commit：`-32603`、`message:"fault.attachment.uploadNotFound"`。

落盘布局（`<data-root>` 相对）：

```text
.mode/cli/artifacts/<session-id>/prompt-attachment-upload-<upload>-tool-result-<id>.txt
.mode/cli/artifacts/<session-id>/prompt-attachment-upload-<upload>-tool-result-<id>.txt
.mode/cli/artifacts/<session-id>/<tool-call-id>-tool-result-<id>.json
.mode/cli/image-cache/<session-id>/image-<id>.png
.mode/cli/debug/model-io-<session-id>.jsonl
.mode/cli/exec/<session-id>/<tool-call-id>-stdout.log
.mode/cli/exec/shell-snapshots/snapshot-bash-<time>-<id>.sh
.mode/v2/telemetry-state.json
```

### 5.2 带附件的发送

- `v4/command sendText` 带 `attachments:[{ref,fileName,mime,bytes}]`：
  ACK `status:"failed"`、`reasonCode:"fault.command.executionFailed"`、
  `message:"FOREIGN KEY constraint failed"`、`revisionAtDecision:0`。
  **这是 oracle 缺陷**（夹具已登记）；Rust 不应复刻，C4 T4 需用户裁决「修」还是「对齐」。
- 同一 ref 经旧 `session/send {content, attachments:[{ref}]}`：应答
  `{accepted:true,sessionId,stateRevision:3}`；turn 正常完成，但 `rowsRange` 的
  `userInput.attachments` 为 `null`——旧通道接受 record 但不把 ref 挂到 userInput 行。
- `v4/commands/query` 对失败命令回显：
  `{key:{sessionId,commandId},result:{commandId,status:"failed",reasonCode,
message:"FOREIGN KEY constraint failed",revisionAtDecision:0}}`。

### 5.3 读面授权错误

对已 commit 的 ref：

| 方法                             | 错误                                                                                               |
| -------------------------------- | -------------------------------------------------------------------------------------------------- |
| `v4/attachment/read`             | `-32603`、`data.name:"Error"`、`message:"fault.attachment.previewRefNotAuthorized"`                |
| `v4/attachment/previewSource`    | 同上                                                                                               |
| `v4/conversation/attachmentStat` | `-32603`、`data.name:"ModeAttachmentFaultError"`、`code:"fault.attachment.shareStatNotAuthorized"` |
| `v4/conversation/attachmentRead` | 同上，`code:"fault.attachment.shareReadNotAuthorized"`                                             |

PNG 的 read/previewSource 与 text 相同。旧 `session/read` 的 parts 仍是
`timeline / text / step-start+text+step-finish`，不内联附件字节。

### 5.4 v4 会话订阅帧

`v4/conversation/subscribe {topic:"conversation/<sessionId>",connectionId,clientMode:"desktop-continuous"}`：

```json
{
  "subscriptionId": "sub-<log-epoch>-1",
  "mode": "snapshot",
  "logEpoch": "<log-epoch>",
  "openTiming": {
    "version": 1,
    "initialFrameEncodeMs": "<time>",
    "sessionRuntimeState": "warm",
    "snapshotRowCount": 0
  }
}
```

`v4/conversation/frame` 通知信封（`wireVersion:3`）：

```json
{ "wireVersion": 3, "kind": "complete", "deliveryKind": "initial",
  "logicalFrameId": "<subscription>-lf-<n>", "logicalFrameOrdinal": 1,
  "topic": "conversation/<session-id>", "subscriptionId": "sub-<log-epoch>-1",
  "frame": { "topic": "conversation/<session-id>", "subscriptionId": "sub-<log-epoch>-1",
    "sentAt": "<time>", "fromSeq": 0, "toSeq": 0,
    "payload": { "kind": "snapshot", "snapshot": { … } } } }
```

- 观察到 `payload.kind` 两类：`snapshot`（首帧）与 `deltas`；信封 `kind` 恒 `"complete"`；
  `deliveryKind` 观察到 `initial` / `online` / `recovery`。
- snapshot 键（21 个）：`availability, backgroundWorks, config, control, goal, inputRouting,
logEpoch, meta, modelTransition, pendingCommands, pendingInteractions, plan,
protocolVersion, queue, revision, rows, seq, sessionId, subagents, usage,
workspaceHookAdmission`。
- 首帧 snapshot：`protocolVersion:1`、`seq:0`、`revision:0`、`control.phase:"draft"`、
  `availability`（fork/compact/switchModelConfig/… 的 allowed+reasonCode）、
  `inputRouting.mode:"startNow"`、`meta:{title:"",titleSource:"default"}`、`rows:[]`。
- delta 项形状：`{op,row}` 与 `{op,patch}`（`deltaOps` 只观察到这两类键组合）；
  row 的 `kind` 未在夹具中收敛（`deltaRowKinds` 为空集时说明本帧只有 patch）。
- **帧数量不稳定**：两次运行分别观察到 20 / 21 / 19 帧（首帧是否已到达、空 delta 帧是否
  合并都会变化）。夹具只记录 `payloadKinds/envelopeKinds/deliveryKinds/paramsKeys/
snapshotKeys/snapshotSample/deltaOps/deltaKeys`，不记录计数。
- `resync {topic,connectionId,subscriptionId,base:{logEpoch,seq}}` →
  `{ack:{subscriptionId,mode:"resume",logEpoch}}`；
- `unsubscribe` → `{}`；
- `sessions-index/<path>` 订阅 → `{ack:{subscriptionId:"<sessions-index-subscription>",
mode:"snapshot",logEpoch:"<sessions-index-log-epoch>"}}`（只录 ACK；未观察 sessions-index 帧内容）。

### 5.5 只读 query 族

- `v4/conversation/rowsRange`：`{rows,atSeq,atRevision,atLogEpoch,hasMore}`；
  行 `kind` 观察到 `turnHeader` / `userInput` / `assistantText` / `toolCall`；
  `userInput` 键：`rowId,turnId,entityId,productTurnId,visibility,createdAt,createdAtSeq,
kind,text,origin`；`toolCall` 键：`assistantResponseId,rowId,turnId,entityId,
productTurnId,visibility,createdAt,createdAtSeq,kind,toolCallId,toolName,status,inputText,
input,output,startedAt,endedAt`。
  分页参数 `beforeRowId` 生效；边界返回 `hasMore:false`。
- `v4/conversation/plans`：`{plans:[],atSeq,atLogEpoch}`（本次无 Plan Mode 产物）。
- `v4/conversation/usage`：`sessionId,totalTokens,inputTokens,outputTokens,reasoningTokens,
cacheCreationTokens,cacheReadTokens,modelRequestCount,modelErrorCount,inputBaselineBySource`。
- `v4/usage/stats {range:"7d"}`：`range,generatedAt,timeZone:"UTC",source:"agent-db",
summary{20 字段…}, heatmap{startDate,endDate,maxTokens,weeks[]},
dailyModelUsage,models,tools`；日期随运行日滚动（夹具折叠 `<range-start>/<range-end>`）。
- `v4/conversation/fileChanges`：本次参数组合返回
  `-32603、message:"proto.staleLogEpoch"`（成功形状未录到）；
- `v4/conversation/backgroundBashOutput`：未取到 workId（探针正则未命中），记为未观察。

### 5.6 DB 与落盘

`attachments-v4-frames` 结束时：`session 1`、`message 13`、`part 27`、
`session_input 1`、`session_entry 4`、`session_target 0`、`session_task_link 0`、
`tool_usage 2`、`turn_usage 5`、`model_usage 7`、`input_history 4`。
`session_input` 行与 `input_history` 一起构成输入审计；具体列形状见夹具 `dbTables`。

### 5.7 附件/v4 未观察到

- `sendText` 带附件成功路径（oracle 仅录到 FK 失败）；
- `attachmentRead/previewSource` 的**成功**字节/预览内容（只录到未授权错误）；
- `fileChanges` 成功形状、`backgroundBashOutput` 成功形状；
- sessions-index 帧内容、`web-remote-replayable` 的 gap/resync 语义；
- 附件大小上限、分片边界、MIME 不支持、文件名冲突；
- `v4/conversation/subscribe` 带 `includeSnapshot:false` 或 resume 起始游标的形状。

### 5.8 对 T4 的直接约束

- 附件 ref 一律 `mode-artifact://<session>/tool-result-<uuid>`；落盘走
  `<dataRoot>/.mode/cli/artifacts/<session>/`，PNG 另进 `image-cache`。
- 授权错误码必须区分 `previewRefNotAuthorized` / `shareStatNotAuthorized` /
  `shareReadNotAuthorized` / `uploadNotFound`。
- 订阅 ACK 与帧信封字段（`wireVersion:3`、`logicalFrameId/Ordinal`、`frame.payload.kind`）
  按 §5.4；snapshot 顶层 21 键与首帧取值按夹具。
- 帧计数不得作为断言；`desktop-continuous` 与 `web-remote-replayable` 的语义分开验证。
- `sendText` 带附件的 FK 失败是 oracle 缺陷：Rust 实现前需用户裁决，不要默认复刻。

## 6. automation / provider / account（`automation-stdio-surface` / `provider-account-stdio-surface`）

automation 协议镜像的 5 个方法（`create/update/checkTaskBinding/list/delete`）在 stdio
agent 面全部 `-32601 Method not found`：

```json
{ "code": -32601, "message": "Method not found: automation/create" }
```

调用顺序 create → update → checkTaskBinding → list → delete 后，普通 turn 仍正常完成
（`turn.completed resultType:"success"`），会话状态未受影响。

provider 2 个方法在 oracle 的 stdio 面**是可用的**：

- `provider/testModelConnectivity {workspace,selection:{providerId:"oracle-mock",modelId:"mock-1"}}`
  → `{success:true}`（确实打到隔离根里的 mock 端点）；
- `provider/updateAccountConfig {revision,basedOnModeBuiltinRevision,providers:{},states:{}}`
  → `{receivedRevision:"probe-account-revision-1",providerCount:0,status:"received"}`。
  该调用会替换进程级 Account Provider Config，因此探针把它放在最后，之后不再发模型请求。

**裁决保持**：`host-ownership-switch.md` 决策 4（automation 10 个 Host 聚合方法
`not-needed`，Renderer 分调两个 owner）与决策 2（provider/account 2 个网络方法在 Rust
频道显式 `unsupported_method`）不因本观察改变。本夹具只证明：
（1）agent stdio 面确实没有 automation 方法，Renderer 不能把 automation 调用路由到 agent；
（2）provider/account 在 Node 面可用，但属 Host/设置面，不是会话内必须行为。

### 6.1 未观察到

- automation 的 Host 侧 10 个聚合方法（不在 stdio 面，本文不录）；
- provider/updateAccountConfig 带非空 providers/states 的合并语义；
- testModelConnectivity 的失败分类（只观察到 oracle-mock 成功）；
- 账号凭据/团队权益/缓存 Key 的真实路径（需要真实凭据，不在 T0）。

### 6.2 对 T5 的直接约束

- automation 5 个协议方法在 Rust 保持 `-32601`（或与 C1 既有 method-not-found 一致）；
  不要为它们写第二份 Host 状态。
- provider/account 保持显式 `unsupported_method` 与批准文案（决策 2）；
  若未来要把 Renderer 的 provider 调用路由到 agent，需要先改决策，不能由本夹具推导。

## 7. 子代理执行（`subagent-execute-foreground`）

### 7.1 Agent 工具定义（父请求逐字）

父请求工具目录 21 个：
`Agent, AskUserQuestion, Bash, CronCreate, CronDelete, CronList, CronUpdate, Edit,
EnterPlanMode, ExitPlanMode, Read, Skill, TaskOutput, TaskStop, TodoRead, TodoWrite,
WebFetch, Write, SendMessage, ReadSessionContext, mcp__node_repl__js`。

`Agent` 工具（`input_schema`，`$schema` 为 2020-12）：

```json
{
  "type": "object",
  "properties": {
    "description": {
      "type": "string",
      "description": "A short (3-5 word) description of the task"
    },
    "prompt": { "type": "string", "description": "The task for the agent to perform" },
    "subagent_type": {
      "type": "string",
      "description": "The type of specialized agent to use for this task"
    },
    "run_in_background": {
      "type": "boolean",
      "description": "Set to true to run this agent in the background. You will be notified when it completes."
    }
  },
  "required": ["description", "prompt"],
  "additionalProperties": false
}
```

`description` 1946 字符，含 agent 类型清单（`general-purpose` Tools:`*`、`Explore`
Tools:`Read, Bash, WebFetch, WebSearch, TodoWrite`）与使用规则；全文见夹具
`expect.agentTool.description`。

### 7.2 Child 请求与父请求的差异

同一 turn 的 3 次模型请求（0=父首请求、1=child、2=父收尾）：

| 项                                 | 父请求                                                  | child 请求                                                                                                                                              |
| ---------------------------------- | ------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `toolCount`                        | 21                                                      | 14                                                                                                                                                      |
| 工具差异                           | 有 `Agent,Cron*,EnterPlanMode,ExitPlanMode,SendMessage` | 无这些；**多一个 `RespondToCoordinator`**                                                                                                               |
| child 工具全集                     | —                                                       | `AskUserQuestion,Bash,Edit,Read,Skill,TaskOutput,TaskStop,TodoRead,TodoWrite,WebFetch,Write,RespondToCoordinator,ReadSessionContext,mcp__node_repl__js` |
| `system`                           | 3 段：41 / 1218 / 5404 chars                            | 4 段：41 / 1245 / 813 / 256 chars                                                                                                                       |
| system 内容                        | `You are Mode…` + 主系统提示 + 沟通规范                 | `You are Mode…` + **agent 系统**（`You are an agent for Mode…`）+ notes（cwd/绝对路径/最终回复）+ env 段                                                |
| `messages`                         | user 2 个 text part：currentDate reminder + prompt      | user 3 个 text part：currentDate + skills reminder + child prompt                                                                                       |
| `model/max_tokens/stream/thinking` | `mock-1/8192/true/{type:"disabled"}`                    | 相同                                                                                                                                                    |

结论：child 不是「复用父 system + 改 prompt」，而是换用 agent 专用 system 与工具子集；
`RespondToCoordinator` 是 child 独有工具。

### 7.3 Child 会话与 turn 事实

- 子会话 `session` 行：`parent_id:"<父 sessionId>"`、`task_type:"subagent_child"`、
  `title:"child execute prompt: inspect only"`（= prompt，不是 description）、
  `slug`=id、`directory`=`<cwd>`。
- 子会话消息（3 行）：`sequence 0` assistant `timeline_event`/part `timeline`；
  `sequence 1` user `user_prompt`/part `text`；`sequence 2` assistant
  `assistant_response`、`finish:"stop"`、parts `step-start,text,step-finish`；
  共 5 个 part。
- 父事件流中只出现 lifecycle 的 `session.updated {agentId,agentType,childSessionId,
description,prompt,parentToolCallId,status,allowedTools}`（running/completed）；
  child 自己的流式/工具事件不混进父流（`general-purpose` 的 `allowedTools:["*"]`）。
- 工具结果逐字模板：

```text
child final text
agentId: agent_<uuid> (use SendMessage with to: 'agent_<uuid>' to continue this agent)
<usage>subagent_tokens: 22
tool_uses: 0
duration_ms: <time></usage>
```

`budgetStrategy:"artifact"`、`truncated:false`、`originalBytes/returnedBytes:228`。

- `session/subagents` ended 项：`childSessionId:"child-1"`、`agentId`、`toolCallId`、
  `subagentType:"general-purpose"`、`title:"exec child"`（description）、
  `startedAt`、`status:"success"`、`summary:"child final text"`、`endedAt`。
- 事件序列包含 `session.titleUpdated`、`turn.started`、`session.updated`、
  `model.streaming`、`tool.updated`（Agent scheduled/started/result/batch）、
  `tool.updated` 的 `streamRecovery.updated` 工具结果锚点、`turn.completed`。

### 7.4 子代理执行未观察到

- child 执行工具（`tool_uses:0`；child 工具循环未驱动）；
- child 失败/取消/超时/`waiting/blocked/lost` 状态；
- 多 child 并发、后台 child 的完整执行窗口（C3 只录到结果消费）；
- `SendMessage` 续聊子代理、`RespondToCoordinator` 的调用行为；
- child session 的独立 `session/read`（C3 已观察结束后返回 `-32004 Session is not active`）。

### 7.5 对 T5 的直接约束

- `Agent` 工具 schema 按 §7.1；child 工具子集不含 Agent/Cron/Plan/SendMessage，
  且包含 `RespondToCoordinator`。
- child system 使用 4 段 agent 专用提示（`You are an agent for Mode…` + notes + env），
  不复用父主提示；child prompt 与 skills reminder 一起进 user 消息。
- child 会话写 `parent_id` + `task_type:"subagent_child"`；结果文本模板与 usage 行按 §7.3。
- lifecycle `session.updated` 的 `allowedTools`：`general-purpose` 为 `["*"]`，
  `Explore` 为具体列表。

## 8. 20 任务清单核对（`host-ownership-switch.md` §7）

| #   | 任务                      | 本波可实现性 / 依赖                                                                                  |
| --- | ------------------------- | ---------------------------------------------------------------------------------------------------- |
| 1   | 新会话发送文本            | C1 已实现；T6 真机复验                                                                               |
| 2   | 多轮上下文 + 中途停止     | C1 已实现；T6 真机复验                                                                               |
| 3   | 切换模型与思考档          | C1 已实现；T6 真机复验                                                                               |
| 4   | 切换会话模式              | C1 已实现；T6 真机复验                                                                               |
| 5   | 关闭后恢复会话            | C1/C2 已实现；T6 真机复验                                                                            |
| 6   | 会话压缩                  | C2 已实现；T6 真机复验                                                                               |
| 7   | 文件读取工具              | C1 已实现；T6 真机复验                                                                               |
| 8   | 文件编辑 + 回滚预览       | C1 编辑已实现；v4 `fileChanges` 成功形状未录到（本次 `proto.staleLogEpoch`），T4 需补参数/补录后接通 |
| 9   | Bash 前台与后台输出       | C1 前台已实现；`backgroundBashOutput` 的 workId 本轮未取到，T4 需补录成功形状                        |
| 10  | 附件上传与预览            | **T4 本波可实现**（事务/授权错误形状已录；`sendText` 带附件的 FK 失败是 oracle 缺陷，需裁决）        |
| 11  | 计划模式产物              | T4 `plans` 查询（空表形状已录）；Plan Mode 产物本身属 LT-5                                           |
| 12  | Goal/Todo 生命周期        | C3 goal 已实现；Todo 读面未专门录（`todo` 表已在 C4 表清单）                                         |
| 13  | conversation 订阅与重同步 | **T4 本波可实现**（subscribe/frames/resync/unsubscribe 已录；帧计数不稳定，按形状断言）              |
| 14  | 历史分页                  | **T4 本波可实现**（rowsRange 行形状/分页已录）                                                       |
| 15  | workflow run 读面         | **T3 本波可实现**（六方法 + run 查询；node/artifact 非空形状未录）                                   |
| 16  | 远程/重连语义             | C2/C3 链路已具备；sessions-index 只在 T4 录到 ACK；`web-remote-replayable` 真机在 T6                 |
| 17  | MCP 列表与状态            | **T1 本波可实现**（stdio/http 连接、工具目录、失败三分类已录）                                       |
| 18  | 插件概览与开关            | **T2 本波可实现**（读面/启停/市场/安装已录）                                                         |
| 19  | Skills 目录与引用         | **T2 本波可实现**（发现/authority/触发/引用解析已录）                                                |
| 20  | 子代理与健康面            | C3 只读 + **T5 执行本波可实现**（child 请求/DB/结果/usage 已录）；健康面既有                         |

补充检查：

- provider/account 两个网络方法返回 `unsupported_method` 与批准文案：**T5 保持裁决**（§6）；
- 从开关移除 `mode-agent` 后回到 Node Host：T6 回滚测试；
- 桌面与手机 Web 各跑 1/13/16：T6 真机环境（需用户凭据/设备）；
- 任务 1/9/10/15 后无孤儿 Agent/MCP 进程：T6/T7 验收。

## 9. 未观察到 / 不可触发汇总

| 面         | 未观察项                                                                                                               |
| ---------- | ---------------------------------------------------------------------------------------------------------------------- |
| MCP        | 真实 OAuth 流程、sse、工具清单变更通知、resources/prompts、权限弹窗、重启重连                                          |
| Skills     | `.claude/skills` 发现、启停/删除、非法 SKILL.md、与 MCP/plugins 的注入顺序                                             |
| Plugins    | 网络 marketplace、hooks、configure/validate/cancelOperation、进度通知载荷、卸载回收时序                                |
| Workflows  | node/actor 真正执行、非空 events/artifacts、失败/恢复、同名/args 冲突、hooks 影响                                      |
| 附件/v4    | sendText 带附件成功、附件读取成功字节、fileChanges 成功、backgroundBashOutput 成功、sessions-index 帧内容、remote 语义 |
| automation | Host 侧 10 个聚合方法（不在 stdio 面）                                                                                 |
| provider   | testModelConnectivity 失败分类、非空 Account Config 合并、真实凭据路径                                                 |
| 子代理执行 | child 工具循环、失败/取消/waiting/blocked/lost、并发/后台完整窗口、SendMessage/RespondToCoordinator 行为               |

## 10. 夹具与复现

| 探针                          | 夹具                                                                        |
| ----------------------------- | --------------------------------------------------------------------------- |
| `probe-c4-mcp.mts`            | `mcp-stdio-happy`、`mcp-stdio-failure`、`mcp-http-oauth-shape`              |
| `probe-c4-skills.mts`         | `skills-catalog-injection`、`skills-trigger`、`skills-reference-resolution` |
| `probe-c4-plugins.mts`        | `plugins-read-face`、`plugins-lifecycle`                                    |
| `probe-c4-workflows.mts`      | `workflows-saved-lifecycle`、`workflows-invalid-files`、`workflows-run`     |
| `probe-c4-attachments-v4.mts` | `attachments-v4-frames`                                                     |
| `probe-c4-automation.mts`     | `automation-stdio-surface`、`provider-account-stdio-surface`                |
| `probe-c4-subagent-exec.mts`  | `subagent-execute-foreground`                                               |

复现要求：

1. 每个探针连续运行两次，`fixtures/c4/` 逐字节相同（2026-11-19 本机已验证全部 14 个）；
2. 隔离断言两条为真、`invalidStdoutLines:0`、`mockRequestPaths` 只有 `/v1/messages`；
3. 不提交真实凭据、用户名、内部地址；临时目录/会话/上传/订阅 id 全部折叠；
4. C1/C2/C3 既有夹具与探针不被改写（C4 只对 driver/support 做可选参数新增）。

## 11. 对 C4 实现的直接约束（汇总）

| 任务 | 约束                                                                                                                                                                                                                      |
| ---- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| T1   | MCP 工具命名 `mcp__<server>__<tool>` / 插件 `mcp__plugin_<marketplace>_<plugin>__<tool>`；`mcp/list` 两模式与三失败分类；结果信封 `success/display/perf/budgetStrategy:"truncate"`；子进程登记                            |
| T2   | skills 只发现 `.mode/skills` + 内置插件；会话 authority fail-closed `-32004`；Skill 结果 `<skill_content>` + `"truncate"`；plugins 读面字段/顺序、未知市场 diagnostics、enable 响应不作事实源、`operationProgress` 非必需 |
| T3   | 工作流文件 `/* mode-workflow` 块注释（不是 YAML）；六方法 + move 的 scope/path；`startSavedWorkflow` ACK 带 `revisionAtDecision/memoryEnabled`；run 事实源 `dwf_*`                                                        |
| T4   | 附件 ref/落盘布局、四个授权错误码、abort 后 uploadNotFound；订阅信封 `wireVersion:3` + 21 键 snapshot；帧计数不作断言；`sendText` 带附件 FK 失败需裁决                                                                    |
| T5   | automation 5 方法 `-32601`；provider/account 保持 `unsupported_method`（决策 2/4）；Agent 工具 schema、child 14 工具子集（含 `RespondToCoordinator`）、agent 专用 system、child DB/结果模板                               |
