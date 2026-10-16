# Spec：Tauri Agent 运行时（M2）

前置：

- 阶段计划 `docs/plans/2026-10-16-tauri-agent-runtime.md`（M2 的任务切分与验收口径）；
- 路线图 `docs/plans/2026-10-09-tauri-rewrite-roadmap.md` 的 M2 节（阶段边界）；
- 纪律 `docs/specs/clean-room-policy.md`（`crates/agent-*/**` 与全部新增代码适用）；
- 数据根 `docs/specs/mode-data-root.md`、隔离硬边界 `docs/specs/dev-data-dir-isolation.md`。

本 spec 是 M2 的实现规格，Task 0 产出。**oracle 一节与「合法差异」一节是后面所有任务的对拍
依据**：Task 1 的协议夹具、Task 2 的只读工具、Task 5 的双链路、Task 7 的对拍矩阵都引用它。

本文档中的「观察（2026-10-16）」= 按 clean-room §黑盒对拍 运行旧实现得到的输入输出与事件序列，
不是读实现得到的结论。命令与字段均为实测。

## 1. oracle 契约

### 1.1 怎么起

对拍用的旧运行时就是现网在跑的 agent 进程，从仓库根启动：

```
node_modules/.bin/tsx apps/mode-cli/packages/cli/src/main.ts app-server --stdio
```

- 计划正文写的 `pnpm --dir apps/mode-cli exec tsx …` **不可用**（tsx 不在该包的 bin 里，
  报 `Command "tsx" not found`）。driver 直接以 `process.execPath` 跑
  `node_modules/tsx/dist/cli.mjs`，不依赖 pnpm 的 PATH shim，也不依赖已构建的 `dist/mode.cjs`。
- 生产口径（`scripts/mode-distribution/runner.mjs` 的 `MODE_AGENT_SERVER_ARGS_JSON`）同样是
  `["…/mode", "app-server", "--stdio"]`，两者只差「跑源码 / 跑产物」。

### 1.2 喂什么、收什么

stdio 是 **NDJSON**：一行一个 JSON 对象。帧形状是 JSON-RPC 风格的四类，**不是** `{type, payload}`：

| 形状 | 含义 |
| --- | --- |
| `{"method":"…","params":{…}}` | 通知（无 id） |
| `{"id":…,"method":"…","params":{…}}` | 反向请求：**CLI 向 host 要东西**，host 必须应答 |
| `{"id":…,"result":{…}}` | 应答 |
| `{"id":…,"error":{"code":…,"message":…}}` | 错误应答 |

server 生命周期（观察）：`session/create` → `session/subscribe` → `session/send`，
会话事件以通知 `session/event` 推送，其 `params` 是会话事件信封
（`{eventId, sessionId, turnId?, seq, timestamp, type, payload?}`，
`type` 取值见 `packages/shared/src/mode-protocol/index.ts` 的 `ModeSessionEventType`）。
`session/create` 的 `result` 是一份会话状态快照，**sessionId 在 `result.session.sessionId`**（顶层没有）。

**driver 层的 `OracleEvent` 是这一层的映射，不是线上形状**：

- `session/event` 通知 → `{type: params.type, payload: params.payload}`；
- 其他方法 → `{type: method, payload: params}`（例如 `startup/storageState`），
  这样启动帧、遥测帧也在同一路事件里可见，不会被静默丢弃。

### 1.3 host 职责（driver 必须扮演）

CLI 会向 host 发反向请求；不应答会让 `session/create` 在 15s 后超时
（观察：`{"code":-32022,"message":"Client request timed out: session/requestRuntimePreferences"}`）。

| 反向请求 | driver 的应答 |
| --- | --- |
| `session/requestRuntimePreferences`（`scope: runtime-materialization \| user-execution`） | `{nativeSearchEnhancementsEnabled:false, memoryEnabled:false, askUserQuestionAutoResolutionEnabled:true, modelContextBudgetStrategy:"preflight-v1"}` |
| `interaction/requestPermission` | `{decision:"allow"}`（对拍只读工具，不测审批；审批三态在 Task 3 单独对拍） |

未列出的反向请求一律回 JSON-RPC 错误（`-32601`），**不静默忽略**：静默忽略会让用例卡到超时，
排查成本远高于一条显式错误。

### 1.4 数据根隔离（硬要求）

对拍**绝不能碰用户真实 `~/.mode`**。观察到的分层事实：

| 变量 | 效果 |
| --- | --- |
| `MODE_DATA_BASE_DIR=<隔离根>` | 版本化数据树（`<根>/.mode/v2/**`）与归属文件 `<根>/.mode/.mode-root.json` 落在隔离根；`~/.mode/v2/**` 不再被写 |
| 仅设 `MODE_DATA_BASE_DIR` | **不够**：CLI 布局（`cli/db`、`cli/log`、`cli/exec`、`cli/artifacts`、`cli/rollout`）仍写真实 `~/.mode/cli/**`（实测：隔离根只有 `.mode/.mode-root.json`，而会话日志出现在 `~/.mode/cli/log/`） |
| 追加 `HOME` / `USERPROFILE`=`<隔离根>` | CLI 布局也落进隔离根（实测：`<根>/.mode/cli/{db,log,plugins}`），真实根不再新增行 |

因此 driver 必须同时注入 `MODE_DATA_BASE_DIR`、`HOME`、`USERPROFILE`（`MODE_HOME` 一并注入，
语义相同），并**断言** `<隔离根>/.mode/.mode-root.json` 在启动后存在——归属文件是
`docs/specs/mode-data-root.md` 定义的「数据根是本次进程的数据目录」的证据。

对拍**不得**继承调用方的 `MODE_PERSONAL_PROVIDER_CONFIG_FILE`：该变量在开发机上指向
真实凭据文件，继承会让对拍打到真实模型并按 token 计费。

### 1.5 模型必须可指定

`session/create` 不传 `model` 时用环境里的默认选择（可能是真实模型）。对拍一律显式传：

```json
{ "workspace": { "workspacePath": "<cwd>", "workspaceKey": "<cwd>" },
  "model": { "providerId": "oracle-mock", "modelId": "mock-1", "options": { "reasoningLevel": "disabled" } },
  "thoughtLevel": "disabled" }
```

观察：缺 `model` → `turn.failed`（`CONFIGURATION_ERROR`，"Select a model before continuing"）；
缺 `options.reasoningLevel` / `thoughtLevel` → `turn.failed`
（`invalid_model_request`，"Reasoning level is required for <provider>/<model>"）。

对拍一律关掉标题生成（`titleGenerationEnabled:false`）：它是一次额外的模型调用，到达时刻不确定，
会让事件流尾部顺序不稳定，且与被测行为（会话循环 / 工具执行）无关。首帧
`session.titleUpdated`（`source:"first_input"`）是本地生成的，不依赖模型，仍然保留。

### 1.7 工具入参在事件流里的位置

实测：`tool.updated` 的 `kind:"scheduled"` 事件**故意不带 `input`**
（`inputOmitted:true` + `inputRef:"model_stream"`），调用入参只出现在
`model.streaming` 的 `kind:"tool_call"` 事件里。第一层（工具调用序列）的入参必须从那里取，
否则会拿 `null` 跟 `null` 比，工具入参整整一层失去意义。

### 1.6 退出

`stop()` 先关 stdin（stdin 关闭时进程正常退出，观察：空 stdin 启动完成后 `exit 0`），
5s 未退出则 kill，再 2s 未退则 SIGKILL。杀不干净会让后续用例抢不到数据根锁。

## 2. 对拍三层口径

同一份输入分别喂旧运行时（oracle）与新运行时，比三层。**比较器只有一份实现**：
TS 侧 `packages/desktop/tests/agent-runtime-oracle/compare.mts`（三个纯函数）。
Rust 侧用例**不实现第二份比较器**——它把自身投影成同一形状后做精确相等断言，两条规则表必然
漂移，而漂移的比较器会让对拍失去意义。

### 2.1 工具调用序列

投影：从事件流按 `toolCallId` 折叠成按「首次出现顺序」排列的 `ToolCall[]`：

| 来源事件 | 取什么 |
| --- | --- |
| `model.streaming` / `kind:"tool_call"` | `input`（入参的唯一来源，见 §1.7） |
| `tool.updated` / `kind:"scheduled"` | `name = toolName`；入参缺失时用上面那份 |
| `tool.updated` / `kind:"result"` | `ok = result.success === true`，`output = result.content` |
| `tool.updated` / `kind:"error"` | `ok = false`，`output = error.message` |

比较规则：**逐项按序比较** `name`、规范化后的 `input`、`ok`、规范化后的 `output`；
长度不等即报差异。`input` / `output` 先过 §3 的白名单规范化（路径、id、时间）。

### 2.2 文件快照

投影：比较时对工作目录做一次快照，`{相对路径 → 内容 sha256}`（隐藏目录与隔离根除外）。

比较规则：两侧键集必须相同；逐键比哈希。**只比内容，不比 mtime / 权限**。
夹具里存的是哈希（16 进制字符串），不是正文——夹具进 git，正文可能含大文件或敏感内容。

### 2.3 事件流

投影：`{type, payload}` 序列，逐事件过 §3 白名单规范化，随后按 §3 的「无序组」规则折叠。

比较规则：**按序逐条比较**；同位置 `type` 不同即报差异；`type` 相同则比规范化后的 payload。
序列长度不同即报差异。

## 3. 不平等的合法差异（白名单）

不给这份白名单，对拍会因时间戳/端口逐条报错，最后被人关掉——所以规则必须显式、有限、可评审。
规范化的唯一实现是 `packages/desktop/tests/agent-runtime-oracle/normalize.mts`，两端都按它做。

| 类别 | 规则 | 理由 |
| --- | --- | --- |
| id（键） | 键名在白名单集合里的值替换为 `<id>`；id 数组键（`toolCallIds`、`committedToolCallIds`、`pendingRequestIds`…）替换为 `<ids>` | 每次运行的 uuid / 会话 id / 工具调用 id 必然不同。集合**显式列举**（`eventId`/`sessionId`/`turnId`/`messageId`/`messageID`/`toolCallId`/`requestId`/`spanId`/`parentSpanId`/`attemptId`/`operationId`/`logicalCallId`/`resultPartId`/`mcpId`…），不用 `.*Id$` 通配——那会连 `providerId` / `modelId` 这类事实键一起抹掉 |
| id（值） | 字符串里出现 uuid → 该 uuid 换成 `<id>`；出现带前缀的随机记号（`msg_`/`part_`/`sess_`/`turn_`/`call_`/`toolu_`/`query_`/`span_`/`trace_` + ≥4 位）→ 换成 `<<前缀>-id>` | 同一个 id 会平铺在正文里（`anchorId` 形如 `msg_xxx_<uuid>:toolu_mock_0_0:tool-result`）。保留 `:tool-result` 这类结构词，只折叠随机部分 |
| 时间 | 键名匹配 `^(timestamp\|duration\|date\|elapsedMs\|.*At\|.*Ms)$` 的值（数值**或** ISO 串）替换为 `<time>` | 时钟必然不同：`startedAt`/`committedAt`/`timestamp` 是 ISO 串，`durationMs`/`timeToFirstTextMs` 是数值，`date` 是 HTTP 响应头的当前时刻 |
| 端口 | 形如 `http://127.0.0.1:<port>` / `ws://localhost:<port>` 的串替换为 `<loopback-url>` | mock provider 端口是 `listen(0)` 取的临时端口 |
| 进程内路径 | 出现隔离根 / 工作目录 / 系统临时目录的前缀替换为 `<data-root>` / `<cwd>` / `<tmp>`；**单反斜杠与 JSON 转义的双反斜杠两种形态都要换** | 每次运行都是新的 mkdtemp 目录；`model.streaming` 的 `delta` 是工具入参的 JSON 文本，里面的路径是双反斜杠 |
| 无序组 | 连续同类型且类型属于 `UNORDERED_EVENT_TYPES`（当前只有 `session.updated`）的一段，折叠成按稳定 JSON 串排序的集合后再比 | 状态推送由不同子系统并发发出，相对顺序不是行为。**其余类型一律按序比**（工具事件的顺序就是调度行为） |
| 环境相关计数 | `toolCount`、`chars` 的值替换为 `<count>` | 工具面与提示词规模取决于机器上装了哪些 MCP / 技能 / 插件的版本，不是被测量行为 |
| 传输层头 | `requestHeaders` / `responseHeaders` 的值整体折叠成 `<headers>`（`requestHeaderCount` / `responseHeaderCount` 保留） | 头里是版本号、平台、时区与当前时刻；HTTP 头是环境事实，不是运行时行为 |

上面没列的一律**逐字比**。要新增白名单，先改本表再改 `normalize.mts`，并在 PR 里说明为什么
差异是合法的而不是缺陷。白名单不足时用例会变**红**（不是静默放过），加规则是一次显式决定。

夹具的第二个纪律：**期望值只能从 oracle 录，不能手写**。手写期望值等于把「我以为的行为」当规格，
正是 clean-room 要避免的；配套的实现约束是 §4.2 的 mock provider——没有它，录下来的期望值
是真实模型的随机输出，同样没有规格价值。

### 3.1 夹具格式

`packages/desktop/tests/agent-runtime-oracle/fixtures/*.json`（行为夹具；Task 1 的**协议夹具**
是另一套，住 `packages/shared/test/fixtures/agent-protocol/`，两者只共享命名与规范化口径）：

```json
{
  "case": "<用例名>",
  "prompt": "<喂给 oracle 的输入>",
  "script": [{ "toolCalls": [{ "name": "Read", "input": { "file_path": "<cwd>/hello.txt" } }] }],
  "expect": { "toolCalls": [], "files": {}, "events": [] }
}
```

- `prompt` 与 `script` 是**输入**（人写）；`expect` 是**期望**（从 oracle 录，人不得手改）；
- `script` 是 mock provider 的剧本（见 §4.2），没有它夹具无法回放；
- 录制入口：`pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/record.mts`；
  录完再跑一次 `pnpm exec oxfmt packages/desktop/tests/agent-runtime-oracle/`（录制器按
  `JSON.stringify` 落盘，格式由仓库格式化器统一）；
- 夹具回放用例会**先重跑 oracle 再比**：手改 `expect` 或改输入都会立刻变红，这是设计意图。

## 4. 对拍可重复性：mock provider

### 4.1 为什么必须有

无 mock 时同一 prompt 两次跑出不同工具序列（实测：真实模型对「读一下 hello.txt」选择了 `Bash`），
用例会随机红，且每次对拍都在真实计费。

### 4.2 形态：本地 HTTP 端点 + 配置指向（不碰 provider 层）

落点不是 `packages/provider-node/src/**`（该目录在上游基线里存在，属上游衍生，clean-room 下只能读
接口面），而是**起一个本地 HTTP 服务扮演 provider 端点，用配置把运行时指向它**：

1. mock 在 `127.0.0.1:0` 上起 HTTP 服务，按 **anthropic-messages** 线协议应答
   `POST /v1/messages`（SSE 流式；`stream !== true` 的请求回一个普通 JSON 报文）；
2. driver 写一份**自己的** personal provider config，把
   `MODE_PERSONAL_PROVIDER_CONFIG_FILE` 指过去，`MODE_BUILTIN_PROVIDER_CONFIG_FILE` 指向仓库内的
   `config/provider/mode-builtin.json`：

```json
{ "schemaVersion": 1,
  "config": {
    "providerConfigRules": { "providerRules": [ {
      "providerId": "oracle-mock", "providerName": "Oracle Mock", "enabled": true,
      "config": { "group": "standard-personal", "visibility": "visible",
                  "access": { "type": "api-key", "apiKey": "oracle-mock-key" },
                  "personalModelIds": ["mock-1"],
                  "api": { "type": "anthropic-messages", "baseUrl": "http://127.0.0.1:<port>" } } } ] },
    "modelConfigRules": { "providerModelRules": [ {
      "providerId": "oracle-mock", "modelId": "mock-1",
      "config": { "properties": { "contextWindow": 200000, "supportsToolCall": true,
                                  "inputFormat": { "supportsText": true }, "outputFormat": { "supportsText": true } },
                  "optionSpecs": { "reasoningLevel": { "values": ["disabled", "enabled"] },
                                   "maxOutputTokens": { "max": 8192 } } } } ],
      "manualProviderModelRules": [] },
    "providerOrder": ["oracle-mock"] } }
```

字段形状取自 `packages/provider/dist/config/rule-data-schema.d.ts` 与
`packages/provider-node/dist/provider-config-file-codec.d.ts` 的**类型声明**（接口面），
不是实现体。

**默认关闭**：`oracle-mock` 只存在于 driver 写出的临时配置文件里，不设
`MODE_ORACLE_PROVIDER`/不指向该文件时，任何日常运行都不会选到它——不新增任何默认可达的代码路径。

### 4.3 确定性口径

mock 的应答是**请求内容的纯函数**，不是计数器也不是随机数：

- `index = 请求 messages 里 role==="assistant" 的条数`（即已经完成几轮）；`index` 落在剧本内取
  `script[index]`，越界则回最后一步；
- 剧本步骤 `{toolCalls:[{name,input}]}` → 生成对应数量的 `tool_use` 块，`stop_reason:"tool_use"`；
  `{text}` → 一个 `text` 块，`stop_reason:"end_turn"`；
- 工具 id 由 `index` 与序号推出（`toolu_mock_<index>_<n>`），同输入同输出；
- 无 `tools` 的请求（标题生成等）回固定文本，不消费剧本；
- `startOracleRuntime` 不传 `script` 时用内置默认剧本（Read `<cwd>/hello.txt` → 一句固定文本），
  只服务「起得来、吐得出事件、退得干净」这类通路用例；**夹具一律显式带剧本**。

### 4.4 自检（本节的验收）

同一 prompt 连跑两次（同剧本、同工作目录），**工具调用序列与应用事件流除白名单字段外逐字相同**。
该用例是 `repeatability.test.mts`；夹具回放用例（`fixtures.test.mts`）同理——录下来的期望值
能再次被同一个 oracle 复现，才算夹具可用。

## 5. 对拍三层之外：不做的事（M2 边界）

| 面 | 归属 | 说明 |
| --- | --- | --- |
| `WebFetch` / `WebSearch` | **M3** | 要网络策略与 sidecar 取回面 |
| `Task` / `Agent` / `Skill` / `js` / `mcp__node_repl__*` | **M3** | 子代理与 node-repl 宿主都在 sidecar |
| `list` | 不存在 | 目录列举由 `Glob` 覆盖，不新造工具 |
| 插件 / MCP / Skills 宿主 | **M3** | 本阶段不搬宿主 |
| UI 替换 | **M4** | M2 只搬运行时 |
| 浏览器自动化 | **M3** | sidecar |
| 审批三态与权限规则表 | Task 3 / Task 6 | Task 0 的 driver 对权限请求一律 `allow`，不把审批行为录进夹具 |
| `mode-protocol/index.ts`（v1）的渲染层历史面 | 不在 M2 范围 | 但**会话面**（`session/*` + `session/event`）就是 oracle 的观测面，见 §1.2 |

工具真源是 `packages/shared/src/tool-identity.ts` 的 `MODE_KNOWN_TOOL_NAMES`（PascalCase）与
`ModeToolFamily` 分档；M2 的只读切片是 `Read` / `Glob` / `Grep`。

## 6. 已知误差与未决

1. **协议面归属**：实测 CLI 的 stdio 会话面是 `session/*` + `session/event`（v1 命名的
   `ModeSessionEventType`）；`v4/**` 方法是 host 面。Task 1 做 v4 镜像时需明确「镜像哪些帧」，
   以本 spec §1.2 的实测形状为起点。
2. **工具总数随机器变化**（实测 `toolCount:191`，含本机 MCP）：夹具不得依赖它，见 §3 白名单。
3. **MCP 子进程启动约 5s**：一次 oracle 会话约 10s，对拍用例应整文件串行、控制用例数量。
4. **HOME 重定向**会影响子进程看到的 `~/.agents/skills` 等用户级目录（隔离后为空），
   故本机夹具的期望值只在「同样的隔离条件」下可复现；换机器需重新录制。
5. **并行批次**：同一个 assistant 消息里的多个工具调用会被运行时并行执行
   （观察：`tool.updated` 的 `scheduled` 带 `canRunParallel:true` 与 `schedule.parallelGroups`），
   它们的 `result` / `batch` 事件到达顺序是竞态——实测同一条夹具两次运行会在
   `result`（alpha/beta）之间换位。夹具因此**避开并行批次**（`read-twice-one-turn` 用两个
   独立的批次表达「同一次 turn 里连续两次工具调用」）。
   要在对拍里覆盖并行调度，需要先在 §3 里为「不同工具调用之间的完成顺序」立一条显式的换位规则
   （只容忍换位、不容忍同一次调用的阶段乱序），再实现它——这是后续任务的显式决定，不是这里的默认值。
   注意：工具调用的**调度顺序**仍然被逐字比较（第一层按 scheduled 顺序、事件流层按序比），
   并行只豁免「谁先跑完」。

## 验收场景（Task 0）

1. `pnpm exec tsx --test packages/desktop/tests/agent-runtime-oracle/driver.test.mts`
   → oracle 能起来、能吐 `turn.started` 与终态、能干净退出；隔离根下存在 `.mode/.mode-root.json`。
2. `repeatability.test.mts` → 同 prompt 两次运行，三层（工具调用 / 文件快照 / 事件流）一致。
3. `fixtures.test.mts` → 3 条夹具（只读一次成功 / 读不存在的文件 / 同 turn 连续两次工具调用）
   全部回放一致；夹具的 `expect` 由 oracle 录制。
4. `compare.test.mts` → 比较器对人为注入的差异报出差异（比较器不能是恒真函数）。
5. `cargo clippy --workspace --all-targets -- -D warnings` 零告警；`pnpm lint` 不高于基线。
