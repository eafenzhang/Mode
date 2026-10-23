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
语义相同），并在启动后**断言两条**隔离事实，任一不成立立刻失败：

1. `<隔离根>/.mode/.mode-root.json` 存在——归属文件是 `docs/specs/mode-data-root.md` 定义的
   「数据根是本次进程的数据目录」的证据；
2. **`<隔离根>/.mode/cli/log` 存在**——CLI 布局的哨兵。

第 2 条才是关键：上表第 2 行说明「只设 `MODE_DATA_BASE_DIR`」时第 1 条**照样成立**，
而 CLI 布局漏到真实数据根。也就是说，如果哪天 `HOME`/`USERPROFILE` 覆盖被删掉，
只断言第 1 条的对拍会**全绿**，同时每次运行都在往用户生产数据根写会话日志与 db。
`cli/log` 由 CLI logger 在进程启动早期创建，它存不存在恰好等价于「HOME 覆盖生效了没有」。

对拍**不得**继承调用方的 `MODE_PERSONAL_PROVIDER_CONFIG_FILE`：该变量在开发机上指向
真实凭据文件，继承会让对拍打到真实模型并按 token 计费。

数据根本身还要过 `assertDataRootIsolated()`：指向真实数据根（含多代旧根 `~/.zcodium`、`~/.zcode`，
见 `docs/specs/p2-mode-naming.md`）时直接抛错。

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

### 1.6 工具入参在事件流里的位置

实测：`tool.updated` 的 `kind:"scheduled"` 事件**故意不带 `input`**
（`inputOmitted:true` + `inputRef:"model_stream"`），调用入参只出现在
`model.streaming` 的 `kind:"tool_call"` 事件里。第一层（工具调用序列）的入参必须从那里取，
否则会拿 `null` 跟 `null` 比，工具入参整整一层失去意义。

### 1.7 退出

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

投影：`{type, payload}` 序列，先按 §3 的**两类无序组**折叠，再逐事件过 §3 白名单规范化
（先后顺序不能反：折叠要读原始 payload 才能分辨「哪几个事件属于同一次工具调用」，
而排序键必须用规范化后的值，否则键里带随机 id、两次运行的排序结果会不同）。

比较规则：**按序逐条比较**；同位置 `type` 不同即报差异；`type` 相同则比规范化后的 payload。
序列长度不同即报差异。被折叠掉的只有 §3 明列的两类顺序（状态推送、并行批次的完成事件）；
其余顺序——尤其是 `scheduled` 的调度顺序与单次调用内部的 `result`/`batch` 先后——逐字比。

## 3. 不平等的合法差异（白名单）

不给这份白名单，对拍会因时间戳/端口逐条报错，最后被人关掉——所以规则必须显式、有限、可评审。

**规则的唯一真源是本表**（`packages/desktop/tests/agent-runtime-oracle/normalize.mts` 是它在 TS 侧的
实现，供 TS 发起的对拍管线使用）。Rust 侧用例**不复制这份实现**：它按本表把自己的事件流投影成
同一形状后做精确相等断言。**只有比较规则（差集算法）是单份实现**——投影必然两端各写一次，
因为两端各自产生事件流；规则表相同、实现分工明确，才不会漂移。

| 类别 | 规则 | 理由 |
| --- | --- | --- |
| id（键） | 键名在白名单集合里的值替换为 `<id>`；id 数组键（`toolCallIds`、`committedToolCallIds`、`pendingRequestIds`…）替换为 `<ids>` | 每次运行的 uuid / 会话 id / 工具调用 id 必然不同。集合**显式列举**（`eventId`/`sessionId`/`turnId`/`messageId`/`messageID`/`toolCallId`/`requestId`/`spanId`/`parentSpanId`/`attemptId`/`operationId`/`logicalCallId`/`resultPartId`/`mcpId`…），不用 `.*Id$` 通配——那会连 `providerId` / `modelId` 这类事实键一起抹掉。〔**Task 3 追加**：`targetMessageId` / `toolMessageId` —— `checkpoint.created` 的回指字段，值与 `messageId` 同类（`msg_<base36 时间戳>_<uuid>`），**前缀不是 uuid 形状**、正文的 uuid 折叠规吃不到它，只能靠键名折叠；与既有的 `anchorId` / `summaryMessageId` 同一条道理。写工具才会产生 checkpoint，所以只读切片时没暴露出来〕 |
| id（容器） | **只对 id 容器键**（`schedule.parallelGroups`、`schedule.executionOrder`）生效：容器里的每个字符串元素换成 `<id>`，**结构与分组保留** | 容器的形状（并行分组数、每组条数、执行顺序长度）是行为，元素身份不是。**不做「见到 `call_xxx` 就当 id」的按形状匹配**：那会误伤工具输出正文（`Read`/`Grep` 结果里出现 `part_number_2`、`call_site_info`、`msg_hello.txt` 是必然的），而正文差异正是 M2 只读工具要对拍的东西 |
| id（值） | 字符串里出现的 **uuid** 换成 `<id>`（一段文本里可能有多处，全部替换） | 同一个 id 会平铺在非 id 键的正文里（如 `anchorId` 形如 `msg_xxx_<uuid>:toolu_x:tool-result`）。uuid 形状是唯一可靠、不会误伤正文的判据 |
| 时间 | 键名匹配 `^(timestamp\|duration\|date\|elapsedMs\|.*At\|.*Ms)$` 的值（数值**或** ISO 串）替换为 `<time>` | 时钟必然不同：`startedAt`/`committedAt`/`timestamp` 是 ISO 串，`durationMs`/`timeToFirstTextMs` 是数值，`date` 是 HTTP 响应头的当前时刻 |
| 端口 | 形如 `http://127.0.0.1:<port>` / `ws://localhost:<port>` 的串替换为 `<loopback-url>` | mock provider 端口是 `listen(0)` 取的临时端口 |
| shell 诊断前缀 | 行首的 `bash: line N:` / `/usr/bin/bash: line N:` / `C:\…\bash.exe: line N:` 折成 `<shell>: line <n>:` | 前缀里的 argv[0] 与行号取决于**怎么启动 bash**（PATH 上的 `bash`、Git Bash 的绝对路径、`-lc` 的包装行数），不是工具行为。正文其余部分（含 `command not found` 的完整句子）仍逐字比。〔Task 4 追加：`bash.rs` 走 PATH/Git Bash，oracle 走 `/usr/bin/bash`，不折这条 `bash-command-not-found` 夹具必红〕 |
| 进程内路径 | 出现隔离根 / 工作目录 / 系统临时目录的前缀替换为 `<data-root>` / `<cwd>` / `<tmp>`；**单反斜杠与 JSON 转义的双反斜杠两种形态都要换** | 每次运行都是新的 mkdtemp 目录；`model.streaming` 的 `delta` 是工具入参的 JSON 文本，里面的路径是双反斜杠 |
| 无序组①：状态推送 | 连续同类型且类型属于 `UNORDERED_EVENT_TYPES`（当前只有 `session.updated`）的一段，折叠成按稳定 JSON 串排序的集合后再比 | 状态推送由不同子系统并发发出，相对顺序不是行为 |
| 无序组②：**并行批次的完成事件** | 同一个 assistant 消息里的多个工具调用会被运行时并行执行；它们的 **完成类事件**（`tool.updated` 的 `result` / `error` / `batch`）到达顺序**不属于契约**。判定必须同时满足：该段是连续同类型 `tool.updated`、**整段都是完成类事件**、且涉及 **≥2 个不同 `toolCallId``**；满足则折叠成排序后的集合。 | 谁先跑完是竞态。**只**豁免这一种情形：调度顺序（`scheduled`）与单次调用内部的 `result`/`batch` 先后仍逐字比——它们才是行为。排序键必须用**规范化后**的 payload，否则键里带随机 id，两次运行的排序结果会不同 |
| 环境相关计数 | `toolCount`、`chars` 的值替换为 `<count>` | 工具面与提示词规模取决于机器上装了哪些 MCP / 技能 / 插件的版本，不是被测量行为 |
| 传输层头 | `requestHeaders` / `responseHeaders` 的值整体折叠成 `<headers>`（`requestHeaderCount` / `responseHeaderCount` 保留） | 头里是版本号、平台、时区与当前时刻；HTTP 头是环境事实，不是运行时行为 |

上面没列的一律**逐字比**。要新增白名单，先改本表再改 `normalize.mts`，并在 PR 里说明为什么
差异是合法的而不是缺陷。白名单不足时用例会变**红**（不是静默放过），加规则是一次显式决定。

**并行批次的实测证据**（2026-10-16）：同一次 assistant 消息里的两个 `Read` 调用，
两次运行的 `result` 事件（`alpha` / `beta`）会互换位置；`scheduled` 顺序始终稳定。
没有无序组②，20 个真实模型任务的验收（阶段验收第 3 项）永远无法判定。

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


### 3.2 `Read` 的入参语义与两道上限（Task 2 实测）

全部出自 `probe-read.mts`（`read` / `trunc` / `limit-edge` / `partial` / `ext` 五组）与行为夹具
`read-offset-edges.json` / `read-truncation-errors.json` / `read-partial-view.json` /
`read-extension-tables.json`。

| 面 | 规则（逐条都有观察） |
| --- | --- |
| 正文形状 | 每行 `"{行号}` + 制表符 + `{该行文本}"`，用换行连接；按换行切段并**保留末尾空段**；每段去掉**行尾一个**回车（行内回车保留） |
| 行号 | `number = offset + 返回序号`，`offset` 缺省 1；起点 = `max(offset-1, 0)`。所以 `offset: 0` 从 **0** 开始编号（怪癖，实测照实现） |
| `limit` | 是**行数**（不是结束行号）；缺省 = 无上限（2500 行的文件一次读完，声明的「up to 2000 lines」与实测不符） |
| `offset` 越界 | **成功**，正文是提醒。判据是 `offset > **真实行数**`，真实行数**不含末尾空段**（`"a
b"`→2、`"a
b
"`→2、`""`→0）：`"a
b"` 配 offset 2 给正文，而 `"a
b
"` 配 offset 3 给 `…shorter than the provided offset (3). The file has 3 lines.`（消息里的行数是**段数**）。空文件另有一支：`offset: 1` 走上面那条，`offset > 1` 走 `…the contents are empty.` |
| 路径 | 相对 `file_path` 按**会话工作目录**解析（声明说必须绝对路径，实测不然）；未知入参键被忽略（声明的 `additionalProperties: false` 没被执行） |
| token 预算 | 估算 `ceil(字节数/3)`；上限 **25000**；判据是**严格大于**；作用在**被选中的范围**（源文本字节数）上，不是整个文件 |
| 超预算：给了范围 | `offset` **键存在且非零**、或给了 `limit` → 错误 `read_output_too_many_tokens`，`message` 里的 token 数就是选中范围的估算值。**`offset: 0` 不算显式**（超预算的单行文件加 `offset: 0` 得到的是部分视图） |
| 超预算：没给范围 | 成功 + **部分视图**：提醒头 + 按行累加「行长+1」直到 **63750** 字节预算（预算窗口由七个样本夹为 [63750, 63754)）；首行自身就超预算时改走另一条提醒头 + 首行截到 63750 个字符（两个样本给出一模一样的正文长度 64043） |
| 文件大小 | **256KB** 上限只在**没给 `limit`** 时生效（给了 `limit: 5000` 的 304999 字节文件走 token 错误），且**先于** token 检查（没给 `limit` 的 302499 字节文件加 `offset: 2` 得到大小错误，尽管按 token 也会超）；`message` 里的 KB 数是**整个文件**的大小 |
| 扩展名 | 二进制**正表**（`.bin`/`.exe`/`.zip`）在读前预检拒绝；图片（`.png`/`.jpg`/`.gif`/`.webp`）属 M2 范围外、返回 fail-loud 标记；**其余一律按文本读**（含没扩展名的 `Makefile` 与没观察过的 `.xyz`——所以「文本」是默认档，不是白名单） |
| 编码 | 含 NUL（或非法 UTF-8）→ `FileSystemPortError` / `unsupported`，`message` = `Unsupported or binary text encoding: {path}` |
| 越界提醒的先后 | 提醒**先于**两道上限（超预算的单行文件加 `offset: 2` 得到提醒、不是 token 错误） |
| **会话内重复读** | 同一会话里**同入参**再读同一文件 → 成功但正文是 `Wasted call — file unchanged since your last Read. Refer to that earlier tool_result instead.`。键 = **解析后的路径 + range**；判「文件变了没有」用 **mtime**（只改 mtime 也给正文；改内容但把 mtime 恢复仍给填充语）；**错误不去重**；跨 turn 仍然命中（状态是会话级的） |

第三层（事件流）对拍在只读切片上**尚未**做：Rust 侧只发工具面事件（见 `crates/agent-core/src/session.rs`），
`turn.completed` 需要的模型层数字与 JS 调用栈不在本任务范围；判据是第一层 + 第二层。

### 3.3 写工具与审批（Task 3 开工前观察，2026-10-17）

全部出自 `probe-write.mts`（`surface` / `call` / `perm` / `project` 四组）与
`probe-write-rules.mts`。**这一节是「先观察再写计划」的产物**——M2 计划 Task 3 的标题与接口
（`Write`/`Edit`/`ApplyPatch` + 四态 `Decision`）有两处与实测不符，逐条列出。

**工具面（声明 ≠ 注册，Task 2 的教训在这里同样成立）**

| 工具 | 对外声明 | 脚本化调用 | 结论 |
| --- | --- | --- | --- |
| `Write` | 在（21 条里） | 成功写入；空入参 → `Tool input failed inputSchema validation` | **已注册** |
| `Edit` | 在 | 未读就写 → `File has not been read yet…`（不是 `tool_not_found`） | **已注册** |
| `ApplyPatch` | **不在** | `{"type":"tool_not_found","message":"Tool not found: ApplyPatch"}` | **未声明、未注册** |
| `MultiEdit` / `NotebookEdit` | 不在 | — | 不存在 |

⇒ **Task 3 的实现范围 = `Write` + `Edit`；`ApplyPatch` 作废**（同 Ruling 17 对 `Glob`/`Grep` 的
处置：实现一个运行时没有的工具等于没有 oracle）。文档标题里的 `ApplyPatch` 与
「多文件与失败回滚」一并作废。

**`Write` 的行为（观察值，不是推断）**

| 场景 | 结果 |
| --- | --- |
| 新建文件 | 成功，正文 `File created successfully at: {path} (file state is current in your context — no need to Read it back)` |
| 覆盖**未读**的已存在文件 | 失败 `tool_execution_failed` / `message`: `File has not been read yet. Read it first before writing to it.` / `detail`: `code=write_file_not_read` |
| 覆盖**已读**的已存在文件 | 成功，正文 `The file {path} has been updated successfully. (file state is current in your context — no need to Read it back)` |
| 入参形状 | `file_path`（必填）、`content`（必填）、`additionalProperties: false` |

「读过没有」是**会话级**的（Task 2 的 `readFileState` 同源）；`Write` 成功后该文件也进入
「已读」状态（正文后半句「no need to Read it back」是这条语义的自述）。

**审批协议（`interaction/requestPermission`，逐字观察）**

请求参数：`{ input, reason, requestId, riskLevel, sessionId, options[], toolCallId, toolName, turnId }`。
`Write` 的 `reason` = `Tool has side effects and requires approval`，`riskLevel` = `medium`。

`options` 有**三条**（不是计划写的四种决策）：

| optionId | kind | response |
| --- | --- | --- |
| `allow_once` | `allow_once` | `{decision:"allow", reason:"Approved once"}` |
| `allow_project` | `allow_always` | `{decision:"allow", reason:"Approved for this project", permissionUpdates:[{type:"addRules", behavior:"allow", rules:[{toolName, ruleContent}]}]}` |
| `deny` | `deny` | `{decision:"deny", reason:"…The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). STOP what you are doing…"}` |

⇒ 计划里的 `AllowOnce | AllowAlways | ProjectAlways | Deny` **四态应改成三态**；`allow_always`
只是 `allow_project` 这条选项的 `kind` 标签，**没有独立的全局一直允许**。

**宿主帧映射（Task 6 Step 5，2026-10-22 落地）**

运行时与 host 之间的审批只走 stdio 反向请求，不另设进程内旁路：

| 方向 | 帧 |
| --- | --- |
| 运行时 → host | `{"id":"server-<n>","method":"interaction/requestPermission","params":<上面的请求参数>}` |
| host → 运行时 | `{"id":"<同一 id>","result":<PermissionResponse>}`，或 `{"id":…,"error":{code,message,data?}}` |

- `id` 由运行时侧**按连接共享**的分配器单调分配（`server-1` 起，与
  `session/requestRuntimePreferences` 的反向请求同一 id 空间）；多个会话/多个反向请求
  发射点必须复用同一个分配器，否则 id 碰撞会让应答被错误的请求认领。应答靠 id 关联，
  不按到达顺序配对。
- `params` 就是上面的请求对象原样序列化（`camelCase`，含 `options[]`）。host 解析失败时按 §1.3
  的失败语义显式回错误，不静默丢弃；TS host 的观察值是 `-32602` /
  `Invalid permission request params`。
- `result` 只要求 `decision`；`reason` / `modifiedInput` / `permissionUpdates` 缺省按 `None`。
- **fail closed**：错误应答、`result` 解码失败、反向通道关闭、序列化失败，一律转成 `deny`
  （理由带上失败事实），不把「没答上来」当成允许。
- 与本请求 id 不匹配的应答帧会被忽略（属于该连接上其他 pending 反向请求）；本处理器不缓存、
  不重放。
- 通道必须是**专用**的：出向只承载反向请求，入向只承载应答；通知帧（`session/event`）走宿主
  自己的事件循环，不进这条通道。
- 本层**不做**规则落库：`allow_project` 的持久化仍由运行时直连 `db.sqlite`（D1 裁决 (a)）。

**落盘（Task 3 Step 1 的产物：格式以 oracle 观察为准）**

`allow_project` **不写任何文件**（`cwd` 与数据根都无新增文件），它写的是运行时 DB 的一行——
`.mode/cli/db/db.sqlite` 的 `local_setting` 表：

| 列 | 值（逐字） |
| --- | --- |
| `scope` | `project` |
| `scope_id` | `proj_` + cwd 路径**小写**、非字母数字的连续段折成单个 `-`（`C:\Users\X\ws` → `proj_c-users-x-ws`） |
| `namespace` | `permission` |
| `key` | `ruleset` |
| `value` | `{"version":1,"allow":[{"toolName":"Write","ruleContent":"<原始大小写路径>"}]}` |
| `schema_version` | `1` |

配套事实：

- 多条 `allow_project` **累积**进同一行的 `value.allow[]`（第二次只更新 `time_updated`）；
- `deny` **不落盘**（ruleset 里只有 `allow[]`）；
- 不同 cwd → 不同 `scope_id`，两行互不污染；
- `permission` 表在本次全部观察里**恒为空**；
- 同 `dataRoot` + 同 cwd 的**第二个会话不再发审批请求**（规则被读出来并生效）。

**累积的边界（2026-10-17 追加探测，两条「探不到」而不是「没问题」）**：

- 每次 `allow_project` 的 `permissionUpdates` 里**恰好一条规则**（`{type:"addRules", behavior:"allow",
  rules:[{toolName, ruleContent}]}`）——「一次勾选带多条规则」这个形态**没有观察**；
- **重复规则不可达**：对同一路径再来一次 `allow_project` 时**审批请求根本不会发**（上一条规则已生效），
  所以「同一条规则会不会被去重」**无法从 oracle 观察到**。复现面：`probe-write-rules.mts` 的
  累积边界组——**必须用全新隔离根**，同一个 dataRoot 下第二次跑会得到「0 次请求」的假观察
  （第一版就踩了，探针里留了注释）。⇒ 实现里对「去重」这件事**不要假装有依据**；
  `agent-core` 侧的规则合并按**追加**实现（与全部观察一致），并把这个未知登记在此。

**风险分级与触发面（`probe-permission.mts`，2026-10-18）**

| 调用 | 是否审批 | `riskLevel` | `reason` |
| --- | --- | --- | --- |
| `Read` | 无 | — | — |
| `Write`（新文件） | 有 | `medium` | `Tool has side effects and requires approval` |
| `Edit` | 有 | `medium` | 同上 |
| `Bash` `echo hi` | **无** | — | — |
| `Bash` `echo a; echo b` | **无** | — | — |
| `Bash` `echo a && echo b` | **无** | — | — |
| `Bash` `sleep 1 &` | 有 | `high` | `High risk tools require explicit approval` |
| `Bash` `rm -rf nonexistent-dir` | 有 | `high` | 同上 |

⇒ **Bash 不是按 family 一律 ask**，而是按**命令模式**分类：模型是「**默认 high + 安全
allowlist**」，不是黑名单。

第二批电池（只读/安全命令）进一步钉住 allowlist：

- **无审批**：`ls` / `ls -la` / `pwd` / `head` / `tail` / `wc` / `find`（不带 `-delete`/`-exec`）/
  `rg` / `git status|diff|log|show|branch|rev-parse` / `which` / `date` / `uname` / `whoami` /
  `printf` / `true` / `sort` / `uniq` / `cut` / `test` / `[` / `echo`（带引号也可）/
  `echo hi 2>/dev/null` / `stat` / `file` / `du` / `ps` / `df` / `grep -r`；
  `|` / `;` / `&&` / `||` 只有在**每一段都安全**时才放行（`cat file1 | head -1`、
  `pwd && echo hi`、`echo hi || true` 都是 none）。
- **high**：文件重定向 `>` / `>>`、`$(...)`、反引号、后台 `&`、`sed -i`、`awk`、`env`、`tee`、
  `xargs`、`eval`、`bash -c` / `sh -c`、`node` / `python` / `npm` / `pnpm`、`cmd`、`curl`、
  `sudo`、`kill`、`find -delete`、`git reset --hard` / `git push`，以及所有写操作命令
  （`mkdir` / `touch` / `cp` / `mv` / `rm` / `chmod`）。
- 已知边界：allowlist 是**观察枚举**；未观察到的安全命令会被多问一次（保守方向）。
  Rust 侧实现见 `crates/agent-core/src/permission.rs`（表驱动用例覆盖上述全部命令）。

**`deny` 的答复形状**（同一探针）

- 事件序列只有 `scheduled` → `permission.requested` → `batch`（`successCount:0` / `errorCount:1`）；
  **没有 `started`、没有 `result`、没有 `error` 帧**。turn 仍是 `resultType:"success"`，
  并继续下一步模型轮（实测 response 为后续文本）。
- `deny` **不落盘**。实现时不要把拒绝做成 `tool_execution_failed`——那是入参校验/执行失败的形状。

**`allow_project` 的匹配范围**

- 规则 `{toolName, ruleContent}` **逐字相等**才命中：同一路径的第二次 `Write` 不再请求；
  不同路径仍请求（实测两次请求分别是 `a.txt` 与 `b.txt`，第二次写 `a.txt` 没有请求）。
- 不做前缀/通配/大小写归一（观察范围内没有这些形态）。

**表结构（黑盒观察，`probe-rule-store.mts`，2026-10-18）**：

```sql
CREATE TABLE local_setting (
  scope text not null,
  scope_id text not null,
  namespace text not null,
  key text not null,
  value text not null,
  schema_version integer not null,
  time_created integer not null,
  time_updated integer not null,
  primary key(scope, scope_id, namespace, key)
)
```

**Rust 侧接线状态（2026-10-18，D1 裁决 (a)）**：`permission::evaluate` 已接进
`run_tool_batch`；allow 走 `started → result`，deny 只留 `scheduled → batch(errorCount 1)`，
`allow_project` 的规则在会话内立即生效，并由 `rule_store::SqliteRuleStore` **跨会话持久化**
（`Session::with_sqlite_rules`）。写入采用 `BEGIN IMMEDIATE` + `busy_timeout(5s)` +
**事务内读-合并-追加**（不是整份覆盖），两个会话并发追加不会互相丢规则；
`rule_store.rs` 有一条 2 线程 × 20 次并发追加的用例。同进程的并发 append 还由
`SqliteRuleStore` 内的 `Mutex` 串行化：Windows CI 上两个短连接同时抢文件写锁会等满
`busy_timeout` 后报 `database is locked`（v0.0.64 发布失败），跨进程仍由 `busy_timeout` 兜底。
写失败记在 `rule_store_error()`。
与 M3 Node Host 同时打开同一 DB 的剩余风险是 **Host 侧长事务**导致的等待，而不是丢更新。

审批的宿主适配已有两半：`ChannelPermissionHandler`（进程内 channel，通道断开按 deny）与
`FramePermissionHandler`（stdio 帧映射，2026-10-22：反向请求/应答与 id 关联，见 §3.3
「宿主帧映射」）；后者负责把 channel 两端接到 `interaction/requestPermission`。

**未决（写进计划，不自行拍板）**：这条持久化归**谁**写？观察里是运行时装进程自己写的，但
目标架构中 DB 由 M3 的 Node Host 侧持有（`node:sqlite`）。Rust 侧是直连 sqlite 还是经 Host 代理，
属架构分叉，需人在 Task 3 子计划里定。

#### 3.3.1 `Edit` 的行为（Task 3 Task B 观察，2026-10-17）

出处：探针 `packages/desktop/tests/agent-runtime-oracle/probe-edit.mts`（17 组，
含专门探「判据先后」的 5 组）。**判据的先后是探出来的，不是推的**——每组只触发一条错误时看不出顺序。

**检查顺序（逐条有观察）**：

| # | 判据 | 答复 |
| --- | --- | --- |
| 1 | `old_string == new_string` | 失败 `No changes to make: old_string and new_string are exactly the same.` / `code=1` / `detail=code=1` |
| 2 | 文件**不存在** 且 `old_string` 非空 | 失败 `File does not exist. Note: your current working directory is {cwd}.` / `code=4` |
| 2' | 文件**不存在** 且 `old_string` 为空 | **成功**（把 `new_string` 当内容**新建**该文件），文案同普通成功 |
| 3 | 文件存在但**本会话没读过** | 失败 `File has not been read yet. Read it first before writing to it.` / `code=6` / `detail=code=6` |
| 4 | `old_string` 为空（文件存在） | 失败 `Cannot create new file - file already exists.` / `code=3` / `detail=code=3` |
| 5 | 子串出现 0 次 | 失败 `String to replace not found in file. String: {old_string}` / `code=8` |
| 6 | 出现 ≥2 次且 `replace_all` 非 true | 失败 `Found {n} matches of the string to replace, but replace_all is false. To replace all occurrences, set replace_all to true. To replace only one occurrence, please provide more context to uniquely identify the instance. String: {old_string}` / `code=9` |
| 7 | 否则 | **成功**，按 `replace_all` 决定替换一处或全部 |

**先后的决定性样本**：`old==new` + 文件不存在 → 拿到 `code=1`（不是 4）；文件不存在 + 未读 → 拿到
`code=4`（不是 6）；文件存在 + 未读 + `old==new` → 拿到 `code=1`（不是 6）；未读 + 串不存在 → 拿到
`code=6`（不是 8）。

**匹配口径**：**纯子串**，不做行/缩进感知——`old_string: "indented"` 在
`"  indented\nnot indented\n"` 上算 **2 次**（匹配到两行的子串），因此报 `code=9`。

**成功文案**（`{path}` 同样是**原样回显入参**）：

- 不带 `replace_all`：`The file {path} has been updated successfully. (file state is current in your context — no need to Read it back)`
- 带 `replace_all`：`The file {path} has been updated. All occurrences were successfully replaced. (file state is current in your context — no need to Read it back)`
  ——**只看 `replace_all` 这个开关，不看实际命中几次**（`replace_all: true` 只命中 1 次时仍是这条）。

`Edit` 与 `Write` 的**同一错误、不同 code**（各自的夹具分开钉，别互相抄）：
「覆盖未读文件」在 `Write` 是 `code=TOOL_EXECUTION_FAILED` / `detail=code=write_file_not_read`，
在 `Edit` 是 `code=6` / `detail=code=6`。

**结果里的 `display.file_diff`（2026-10-18 补观察并实现）**

成功结果的 `display` 是 `Write` / `Edit` 的 diff 呈现来源；**新建 `Write` 没有 display**：

| 工具 / 场景 | `display` | 夹具 |
| --- | --- | --- |
| `Write` 新建 | **无** | `write-new`、`write-relative-path` |
| `Write` 覆盖已读文件 | `file_diff` | `write-overwrite-read` |
| `Edit` 替换一处 / 全部 | `file_diff` | `edit-unique`、`edit-replace-all` |
| `Edit` 空 `old_string` 新建 | `file_diff`（旧文件视为 0 行） | `edit-empty-old-creates` |
| 失败结果 | 无 | 其余 `edit-*` / `write-*` 失败夹具 |

形状（逐字）：

```json
{"kind":"file_diff","filePath":"<cwd>/target.txt","additions":1,"deletions":1,
 "structuredPatch":[{"oldStart":1,"oldLines":3,"newStart":1,"newLines":3,
                     "lines":[" alpha","-Beta","+BETA"," gamma"]}],
 "truncated":false}
```

- `filePath` 是**解析后的绝对路径**，正斜杠形态（Windows 录制也是 `/`）；
- `lines` 每行带前缀（`" "` / `"-"` / `"+"`），不含行尾换行；
- 上下文行数取 unified diff 通行的 3；样本都是小文件，hunk 覆盖全文，无法从样本反推更细的常数；
- 超过单批预算的 `truncated:true` 分支**未观察**，Rust 侧只产出 `false`（与所有样本一致）。

Rust 实现：`crates/agent-tools/src/file_diff.rs`；`Write`/`Edit` 在写成功后调用它，
对拍时 `tool_parity.rs` 逐字段比较 `display`（此前只解析不比较，故该缺口长期未暴露）。

### 3.4 `Bash` 与取消（Task 4 Step 0 观察，2026-10-17）

出处：`probe-bash.mts`（20+ 组）与 `probe-bash-cancel.mts`（运行中取消）。

**输入与输出（逐字）**

| 场景 | 结果 |
| --- | --- |
| `printf 'hello\nworld\n'` | `success=true`，`content: "hello\nworld"`（**末尾换行被去掉**） |
| `exit 7` | **`success=true`**，`content: "Exit code 7"`（非零退出不是工具错误） |
| `printf 'oops\n' >&2` | `success=true`，`content: "oops"`（**stderr 合并进 content**） |
| `printf 'out1\n'; printf 'err1\n' >&2; printf 'out2\n'` | `content: "out1\nerr1\nout2"`（**按到达顺序交错**，不是先 stdout 后 stderr） |
| `printf 'a\n\n\n'` / `printf 'a   \n'` | 都是 `content: "a"`（**`trim_end`：连尾随空格一起去掉**） |
| `true` | `content: "(Bash completed with no output)"`（31 字节） |
| `pwd` | `/c/Users/…`（**Git Bash / MSYS 路径**） |
| `[ -t 1 ]` / `[ -t 0 ]` | `stdout:pipe stdin:pipe`（**不是 pty**，用管道） |
| 不存在的命令 | `success=true`，`content: "Exit code 127\n/usr/bin/bash: line 3: …: command not found"` |
| `sleep 5` + `timeout: 1000` | `success=true`，`content: "Command timed out after 1s\n<error>Command was aborted before completion</error>"` |
| `{}`（缺 `command`） | `success=false`，`type=tool_execution_failed`，`message=Tool input failed inputSchema validation` |

**大输出（持久化包装）**

- 4K / 8K / 16K / 20K / 24K / 28K / **30,000** 字节：内容是原文，`contentLength` 等于原始字节数。
- **30,720** 字节及以上：工具结果是**包装**而不是原文（观察值约 2,251–2,254 字节）：

```text
<persisted-output>
Output too large (32KB). Full output saved to: <dataRoot>/.mode/cli/exec/<sessionId>/<toolCallId>-stdout.log

Preview (first 2KB):
<前 2048 字节>
...
</persisted-output>
```

- 关键口径：`result.originalBytes` / `returnedBytes` 指的是**包装**（约 2.2KB），**不是**命令输出；
  `truncated=false`、`budgetStrategy=artifact`。完整输出落在上面的 `-stdout.log` 文件里。
- **精确阈值**（`probe-bash-threshold.mts`，2026-10-18）：**30,000 字节直出、30,001 字节起持久化**
  （29,999 / 30,000 的正文长度分别就是原字节数；30,001 起变成约 2.2KB 的包装）。
- **`budgetStrategy` 不是只有大输出才是 artifact**：小输出（如 `hello\nworld`）与超时结果的
  `budgetStrategy` 也都是 `"artifact"`（`probe-bash-threshold` / 夹具 `bash-stdout` / `bash-timeout`）；
  `artifactPath` 只在持久化包装与后台任务结果里出现。
- Rust 侧落点接线（2026-10-18）：`ToolContext.artifact` 携带**已解析的 `.mode` 数据根**、
  `sessionId` 与 `toolCallId`；`Session::with_data_root` 接线后 artifact 落
  `{data_root}/cli/exec/{sessionId}/{toolCallId}-stdout.log`（观察路径里的 `<dataRoot>/.mode` 中，
  `<dataRoot>` 是基目录，`.mode` 是这里的数据根）。未接线数据根的会话仍落系统临时目录。

**取消（`probe-bash-cancel.mts`，运行中的 `sleep 30`）**

- 方法：`session/stop`，params `{sessionId}`，应答 `{}`。v1 方法表把它标为 deprecated（已收敛 v4
  `stop`），但 stdio 运行时的线上行为仍是它。
- 运行中的工具得到 `tool.updated` / `kind:"error"`：
  `{"type":"tool_cancelled","message":"Bash was cancelled and the child process was asked to stop","code":"TOOL_CANCELLED"}`。
- 随后是 `tool.updated` 的 `batch`（`successCount:0` / `errorCount:1`）与 `turn.completed`，
  **`resultType:"cancelled"`**（不是 `turn.failed`）。
- **取消后事件流不关闭**（会话仍在），所以「事件流结束」不能当取消成功的判据；
  判据是 `turn.completed.resultType === "cancelled"`。
- 与超时的区别：**超时是工具成功**（两行 content），**取消是工具错误** `TOOL_CANCELLED` + turn cancelled。
  两者不能合并建模。

**超时的进程清理**：`sleep 30 & echo $! > child.pid; wait` 在 1s 超时后，父 shell 被 abort，
但 `kill -0 $(cat child.pid)` 立刻检查是 **`alive`**，2 秒后再查仍是 **`alive`**——
**超时不会清理孙进程**。Rust 侧若额外清理，就是与 oracle 的行为差异，必须显式登记。

**取消的进程清理（2026-10-18 补测）**：同一形状在 `session/stop` 取消后，孙进程在
**+0 / +1s / +2s 都是 `alive`**——**取消同样不清理孙进程**（与超时一致）。
另注：含 `&` 的复合命令会先发 `permission.requested`（`riskLevel:"high"`、`started` 为
`readOnly:false` / `sideEffectScope:"system"`），与本段前面 `sleep 30` 的「无审批」观察不同；
不要按命令形状自行推断是否要审批。

**调度字段（观察值，不按直觉推断）**

- `scheduled`：`canRunParallel:false`、`parallelGroupIndex:0`（Bash 一批一条）。
- `started`：`readOnly:true`、`sideEffectScope:"none"`——**本次观察如此**；同时本次 `Bash`
  **没有**发 `interaction/requestPermission`。不要据此推断权限模型；它只是当前线上的事实，
  Task 4 的实现以这些字段为准。

**并发输入（`probe-bash-admission.mts`）**

上一 prompt 未结束时再发 `session/send`，应答是**错误**而不是排队：

```json
{"code":-32010,"message":"A prompt is already running for this session"}
```

⇒ stdio 会话面**不做排队**。AGENTS.md 里「busy/running 输入由 CLI/runtime `CommandInbox` 串行准入」
的排队层在**这个协议面之上**（host 层）；本探针直接打会话面，观察不到队列顺序。实现前必须
先把分层定清楚（见 Task 4 子计划 DR-T4-7），不能把「会话面拒绝」误当成「CommandInbox 已实现」。

**`run_in_background`（`probe-bash-background.mts`，2026-10-18 Task 4b 前置观察）**

- **返回形状**：立即成功（不等待命令结束），`success=true`，正文逐字（344 字节）：

```text
Command running in background with ID: exec_<uuid>. Output is being written to: <path>. You will be notified when it completes. To check interim output, use Read on that file path.
```

- 结果字段与前台不同：`budgetStrategy:"artifact"`、且**多一个 `artifactPath`**（前台没有这个键）；
  `truncated:false`，`originalBytes` / `returnedBytes` = 正文的 UTF-8 字节数。
  `perf.detail.kind:"command"`、`command.status:"backgrounded"`。
- **落盘**：`<dataRoot>/.mode/cli/exec/<sessionId>/<toolCallId>-stdout.log`；stdout 与 stderr
  **按到达顺序合并**，且**保留尾随换行**（与前台成功结果的 `trim_end` 不同）；
  没有输出时文件**仍存在且为空**（实测两条后台命令的日志分别为 `bg-out\nbg-err\n`、`bad-out\nbad-err\n`）。
- **完成通知不是单发帧，而是自动开一个新 turn**：先 `session.updated`
  `{toolName, toolCallId, taskId}`，再 `turn.started`，其 `input` 是注入的 XML：

```text
<task-notification>
<task-id>exec_<uuid></task-id>
<tool-use-id><toolCallId></tool-use-id>
<output-file><...>-stdout.log</output-file>
<status>completed|failed</status>
<summary>Background command "<description>" completed (exit code 0)</summary>
</task-notification>
```

  非零退出时 `<status>` 是 `failed`，`<summary>` 是
  `Background command "<description>" failed with exit code 7`（逐字）。
- **多任务按完成顺序各起一个 turn**（实测：1s 失败的先、2s 成功的后）；每个通知 turn 都以
  `turn.completed(resultType:"success")` 结束，事件流不关闭。`<description>` 来自入参；
  未提供时的文案**未观察**。
- 旁证：数据根下同时出现 `.mode/cli/exec/shell-snapshots/…` 的 shell 快照文件（运行时既有行为）。

**`TaskOutput` / `TaskStop`（`probe-task-control.mts`，2026-10-18）**

声明面（逐字取自请求体）：

| 工具 | input schema 要点 |
| --- | --- |
| `TaskOutput` | `task_id`（string）、`block`（boolean，default `true`）、`timeout`（number，0–600000，default `30000`）；`required` 列出三项，但**只传 `task_id` 也能通过校验**（有默认值的字段可省略） |
| `TaskStop` | `task_id`（string）、`shell_id`（string，标 deprecated）；无 `required` |

- `TaskOutput` 的成功结果是 **XML 形状**（`status:"completed"` 时）：

```text
<retrieval_status>success</retrieval_status>

<task_id>exec_<uuid></task_id>

<task_type>local_bash</task_type>

<status>completed</status>

<exit_code>0</exit_code>

<output>
<命令输出原文，保留换行>
</output>
```

  `display = {kind:"task_output", retrievalStatus:"success", taskStatus:"completed", output:"<同 output>"}`；
  `budgetStrategy:"artifact"`、`truncated:false`，两个字节数 = 结果正文的 UTF-8 长度（实测 232）。
- **`failed`（非零退出）**：结构同上，`<status>failed</status>`、`<exit_code>7</exit_code>`，
  `<output>` 仍带命令输出（实测 `bad` → display `{…,taskStatus:"failed",output:"bad"}`）。
- **`killed`（被 `TaskStop` 停止）**：`<status>killed</status>`、`<exit_code>137</exit_code>`，
  且**没有 `<output>` 段**；`display` 也**没有 `output` 键**：
  `{kind:"task_output", retrievalStatus:"success", taskStatus:"killed"}`（194 字节）。
  注意同一个任务在 `session.updated` 里的 `status` 是 **`cancelled`**——两个面用词不同，
  不要拿一个去推另一个。
- **`running`（`block:false` 且任务未结束）**：`<retrieval_status>not_ready</retrieval_status>`、
  `<status>running</status>`，且**没有 `<exit_code>` 与 `<output>` 段**；`display` 是
  `{kind:"task_output", retrievalStatus:"not_ready", taskStatus:"running"}`（169 字节）。
- `TaskStop` 的成功结果正文是 **JSON 字符串**（不是 XML）：

```json
{"message":"Successfully stopped task: exec_<uuid> (sleep 30)","task_id":"exec_<uuid>","task_type":"local_bash","command":"sleep 30"}
```

  `display = {kind:"task_stop", taskId, taskType, command, message}`；`budgetStrategy:"truncate"`（193 字节）。
  `started` 载荷是 `readOnly:false` / `sideEffectScope:"session"`。
- **不存在的 id**：两者都是 `tool_execution_failed`，message 同为
  `No task found with ID: <id>`；但 `code` 不同——`TaskOutput` 是 `"2"`（detail `code=2`），
  `TaskStop` 是 `TOOL_EXECUTION_FAILED`（detail `code=1 reason=background_task_not_found`）。
- **TaskStop 的连带副作用**：先 `session.updated` 把任务从 `running`（带 `cancelRequestedAt`）
  改成 `cancelled`（带 `completedAt` / `outputBytes` / `stdoutBytes` / `stderrBytes`）；随后仍注入
  一个 `<task-notification>` turn：`<status>killed</status>`、
  `<summary>Background command "<description>" was stopped</summary>`。
- **调度/启动字段**：两者 `scheduled` 都是 `canRunParallel:true`、`parallelGroupIndex:0`；
  `TaskOutput` 的 `started` 是 `readOnly:true` / `sideEffectScope:"none"`，
  `TaskStop` 是 `readOnly:false` / `sideEffectScope:"session"`。
- **失败发生在 started 之前还是之后**：`TaskOutput` 找不到任务时**没有 `started`**
  （`scheduled` → `error`，说明查找是启动前的前置检查）；`TaskStop` 找不到任务时
  **有 `started`**（`scheduled` → `started` → `error`）。
- **缺 `task_id`**：`TaskOutput` 在入参校验阶段失败——`tool_execution_failed` /
  `Tool input failed inputSchema validation` / code `TOOL_EXECUTION_FAILED`，
  **同样没有 `started`**；`TaskStop` 在 handler 内失败——`tool_execution_failed` /
  `Missing required parameter: task_id` / code `TOOL_EXECUTION_FAILED` / detail `code=1`，
  **有 `started`**（与上面的阶段差异一致）。

**Rust 侧实现状态（2026-10-18）**：`TaskRegistry` + `TaskOutput` / `TaskStop` 已落地，
`Bash(run_in_background)` 会登记任务并按上面的文案返回；`completed` / `failed` / `killed` /
`running` 四种结果形状均已按观察实现。完成通知的**自动调度**尚未实现：
`Session::run_pending_notifications` 已可用，但需要调用方在空闲时触发（Task 5 的完整循环）。

**仍未观察（Step 0 剩余）**：host 层 `CommandInbox` 的跨层行为；排队顺序与 key 粒度按下面的
设计接口先行落地，跨层对拍等 host 面可观察后再补（DR-T4-7 已裁决 (c)）。

### 3.4.1 `CommandInbox`（host 层队列，设计接口，2026-10-18）

**这不是 oracle 观察值**。stdio 会话面只观察到并发 prompt 被拒绝（`-32010`
`A prompt is already running for this session`，不排队）；host 层队列是为 AGENTS.md
「已接受的 busy/running 输入串行准入」提供的独立接口，跨层对拍等 host 面可观察后再补。

语义（`crates/agent-core/src/command_inbox.rs`）：

- 调用方传 `key`（会话键；本地/远程的粒度由 Host 决定），**同 key FIFO**，不同 key 互不影响；
- 第一个 `admit` 立即 `Started`；其余 `Queued { position }`（从 1 起，按到达顺序）；
- `complete(key, id)` 只接受当前 running 的 id：成功则提升队首并返回其 id（`Promoted`），
  没有排队项返回 `Idle`；**stale 完成**（id 不匹配）返回 `Stale`，**不改变队列**；
- `cancel_queued(key, id)` 只移除排队项；running 的取消走 `complete`（oracle 的取消也以
  终态收口，见 §3.4 的 `turn.completed(resultType:"cancelled")`）；
- **不设超时**：准入顺序由调用方同步推进，禁止用超时掩盖同步问题（AGENTS.md）。

用例 `crates/agent-core/tests/command_inbox.rs` 直接断言准入顺序（不断言等待时长）。

### 3.5 会话流与后台通知准入（Task 5 Step 0 观察，2026-10-18）

出处：`packages/desktop/tests/agent-runtime-oracle/probe-session-stream.mts`（三个用例）。

**纯文本 turn 的事件顺序**（逐条，`desktop-continuous`）：

```text
session.titleUpdated
turn.started
session.updated（messageCount=6）
session.updated（provider/model/请求元信息）
model.streaming（kind:"text_delta"）
session.updated（空载荷）
session.updated（content、stopReason:"stop"、toolCallCount:0）
turn.completed（resultType:"success"、tokenCount、usage）
```

- **`turn.started.turnNumber` 第一个是 `0`**（不是 1），之后逐次 +1；Rust 侧已修正并有单测
  `crates/agent-core/tests/session_turn.rs`。
- `turn.completed` 的 `usage` 键（10 个）：`source` / `modelRequestCount` / `inputTokens` /
  `outputTokens` / `totalTokens` / `cacheReadTokens` / `cacheWriteTokens` / `reasoningTokens` /
  `webFetchRequests` / `webSearchRequests`。
- 本机从 `turn.started` 到 `model.streaming` 约 5s，是 MCP 启动开销（§6.3），不是会话语义。

**工具 turn 的插入位置**（Read 为例）：

```text
… model.streaming（tool_input_start → tool_input_delta → tool_input_end → tool_call）
tool.updated（scheduled）→ tool.updated（started）
session.updated（assistant 消息：stopReason:"tool-calls"、toolCallCount:1）
tool.updated（result）→ tool.updated（batch）→ streamRecovery.updated
session.updated（messageCount=8）
… model.streaming（text_delta）→ turn.completed
```

- **长跑 `Bash` 每约 1s 发一条 `tool.updated` / `kind:"progress"`**，载荷逐字（一次实测）：

```json
{"toolCallId":"toolu_mock_2_0","toolName":"Bash","elapsedMs":3049,"pid":21596,"stdoutBytes":0,"stderrBytes":0,"outputBytes":0,"outputPreview":{"text":"","fullText":"","totalLines":0,"totalBytes":0,"linesEstimated":false},"stdoutTail":"","kind":"progress"}
```

  `progress` 已在 Rust 镜像建模（`ToolProgressPayload`，往返测试用上面这条样本）；
  `raw` 仍未建模（出现时应解析失败，留给后续任务）。

**后台通知与运行中 turn 的相遇（DR-T5-3 的依据）**

- 后台任务在**另一个 turn 运行期间**完成：`session.updated {status:"completed"}` 照常发出，
  但**没有产生通知 turn**（该 turn 结束后空闲等待 20s 仍无）。
- 对照（§3.4）：会话**空闲**时同一场景会立刻开通知 turn。
- 结论：oracle **不排队，直接丢弃**。M2 对拍应按此实现；若产品要求排队，属**有意差异**，
  需单独 spec 批准，不能默默改掉。

**重放类型盘点（只读 `packages/shared` 声明面）**

| 类型 | 关键字段 |
| --- | --- |
| `TaskStreamMirrorBatchEvent` | `taskId`、`runId`、`batchSeq`、`fromSeq`、`toSeq`、`ops[]`、`terminal` |
| `TaskStreamMirrorOp` | 每条带 `seq`；`kind:"user_message"` 或 `kind:"stream_event"` |
| `TaskStreamWatermark` | `runId`、`opSeq` |
| `TaskSnapshotInvalidatedEvent` | `taskId`、`reason`、可选 `streamWatermark` |

这些类型的所有者是 **Host/relay 层**（注释点名 desktop `taskRealtimeBus`/`taskRealtimeBridge`、
services `sessionRealtimePort`）；stdio 运行时只发 `desktop-continuous`。M2 **不复制**快照/队列，
只透传 `deliveryKind`（DR-T5-1 裁决 (a)）。

**两端的验证面（DR-T5-1 落地后）**：

- Rust 侧：`crates/agent-core/tests/turn_stream.rs` 的
  `events_carry_the_requested_delivery_kind` 断言每个事件信封都带请求的 `deliveryKind`
  （`web-remote-replayable` 用例）；sink 是 push-only，晚订阅者拿不到历史，没有补发路径。
- Host 侧：`packages/desktop/tests/task-realtime-replay.test.mts` 锁住 main 进程
  `TaskRealtimeBus` 的重放语义——flush 后接入的 Host 收到同 seq 流的
  `deliveryPurpose: "observer"` 重放；批次未 flush 时接入收到
  `task_snapshot_invalidated` / `stream_mirror_gap` 而不是断流；终态批次 flush 后删除，
  不再产生过期重放。改 stream / snapshot / queue 时两侧测试必须一起跑。

复现命令：`pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-session-stream.mts`。

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

**默认关闭**：`oracle-mock` 只存在于 **driver 写出的那份临时 personal provider config** 里。
运行时之所以会选到它，仅仅因为 driver 把 `MODE_PERSONAL_PROVIDER_CONFIG_FILE` 指向了那份文件；
不指向它时（日常开发、桌面、CLI 的任何正常路径）provider 清单里根本没有 `oracle-mock`，
不新增任何默认可达的代码路径。**没有 `MODE_ORACLE_PROVIDER` 这类开关**——
配置指向就是全部机制（见 §4.2 第 1、2 步）。

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

〔**Task 2 观察，2026-10-16，待裁决**〕上面那句「只读切片是 `Read` / `Glob` / `Grep`」**与实测不符**：
`MODE_KNOWN_TOOL_NAMES` 是**渲染用**的名字表（UI 渲染历史会话的工具行要用它），不是运行时的注册表。
按 clean-room 的黑盒对拍（探针 `packages/desktop/tests/agent-runtime-oracle/probe-tool-surface.mts`
的 `tools` / `absence` 两组，命令与原始输出见该文件头部）：

- 运行时在 stdio 会话面**固定声明 21 条工具**，只读切片里**只有 `Read`**；
- 脚本化调用 `Glob` / `Grep` 的答复是
  `{"type":"tool_not_found","message":"Tool not found: Glob","code":"TOOL_NOT_FOUND"}`——即**未注册**；
- 那 21 条不随 `--surface desktop`、`--mode build|plan`、`--disallowed-tools`、
  `nativeSearchEnhancementsEnabled` 变化（正对照：`--disallowed-tools Bash` 下 `Bash` 照样被声明）；
- 一次会话里 `session/requestRuntimePreferences` 只出现两次（`runtime-materialization` 与
  `user-execution` 各一次），没有第三个偏好可开关工具面。
- **上面这几条已经自动化**：`packages/desktop/tests/agent-runtime-oracle/tool-surface.test.mts`
  起两次会话（默认参数 / `--surface desktop --mode build --disallowed-tools Bash`），断言两次
  声明的名字集合**逐字相等**、且 `Bash` 两次都在（正对照）、`Read` 在、`Glob`/`Grep` 不在。
  跑它：`pnpm exec tsx --test packages/desktop/tests/agent-runtime-oracle/tool-surface.test.mts`
  （它也在 `pnpm test:agent-runtime-oracle` 里）。探针侧的复现入口是
  `probe-tool-surface.mts` 的 `MODE_PROBE_CLI_ARGS`。

因此 `crates/agent-tools` 的注册表**只注册 `Read`**，`Glob`/`Grep` 的正确行为是「未注册 → 与
oracle 同形的 `tool_not_found`」，并由行为夹具 `read-absent-search-tools.json` 录下证据。
**这是产品口径问题，不在实现里自行决定**：建议 ledger 裁一条——M2 只读切片收窄为 `Read`
（`Glob`/`Grep` 另找协议面或另立项）。另有一个未探索的分支供参考（**只从文件名得知、未打开实现体**）：
`apps/mode-cli/packages/core/src/{tool/handlers/glob.ts,subagent/explore-tools.ts,tool/provider-visible-order.ts}`
存在，`Glob`/`Grep` 可能属**子代理（explore）**的工具集，而子代理在 spec §5 里归 M3。

## 6. 已知误差与未决

1. **协议面归属**：实测 CLI 的 stdio 会话面是 `session/*` + `session/event`（v1 命名的
   `ModeSessionEventType`）；`v4/**` 方法是 host 面。Task 1 做 v4 镜像时需明确「镜像哪些帧」，
   以本 spec §1.2 的实测形状为起点。
2. **工具总数随机器变化**（实测 `toolCount:191`，含本机 MCP）：夹具不得依赖它，见 §3 白名单。
3. **MCP 子进程启动约 5s**：一次 oracle 会话约 10s，对拍用例应整文件串行、控制用例数量。
4. **HOME 重定向**会影响子进程看到的 `~/.agents/skills` 等用户级目录（隔离后为空），
   故本机夹具的期望值只在「同样的隔离条件」下可复现；换机器需重新录制。
5. **并行批次的完成顺序是竞态**（不是缺陷）：同一个 assistant 消息里的多个工具调用会被并行执行
   （观察：`tool.updated` 的 `scheduled` 带 `canRunParallel:true` 与 `schedule.parallelGroups`），
   实测同一条输入两次运行会在两条 `result`（alpha/beta）之间换位，而 `scheduled` 顺序始终稳定。
   处置见 §3 无序组②：只豁免「谁先跑完」，调度顺序与单次调用内部的阶段先后仍逐字比。
   夹具里的 `read-twice-one-turn` 保留**单元素** `parallelGroups`（两个独立批次），
   因为那是最贴合「同一次 turn 里连续两次工具调用」这条要求、且不依赖竞态的形状；
   并行批次的折叠规则由 `compare.test.mts` 的纯函数用例覆盖。
6. **已修复缺陷（2026-10-24 全部收口）：v1 宿主 schema 曾落后于线上，host 会丢掉两类会话事件。**
   （Task 1 发现，2026-10-16 复评确认；本项**不在 Task 1 修**，需独立调查与决定。）
   实测证据（两条真实帧 + 跑一遍 zod 即可复现）：运行时发出的
   `turn.started` 载荷带 `executionStartedAt`（小数毫秒），
   `tool.updated` / `kind:"started"` 带 `readOnly` 与 `sideEffectScope`；
   而 `packages/shared/src/mode-protocol/index.ts` 的对应载荷 schema 没有声明这些字段，
   且各成员都是 `.strict()` ⇒ `modeSessionEventSchema.safeParse(真实帧)` 失败，
   报 `unrecognized_keys`。宿主路径用同一个 `modeSessionEventSchema` 校验 `session/event`，
   失败即记日志并丢弃整条事件——**每一次对应事件都被丢掉**（不是"文档不准"，是线上丢事件）。
   两半边均已于 2026-10-24 修复：
   - **`turn.started` / `executionStartedAt`**——schema 补声明
     `executionStartedAt: z.number().nonnegative().optional()`（刻意不用 int-only 的
     `timestampMsSchema`：CLI 发的是 `performance.timeOrigin + performance.now()` 的
     **浮点**毫秒，int schema 会换一种方式重复同一起丢弃事故）；端到端证据：
     stream-latency-host bench 的「丢弃无效 Mode Protocol session event」日志清零。
   - **`tool.updated`/`started`（`readOnly` / `sideEffectScope`）**——started 变体补声明
     `readOnly: z.boolean().optional()` + `sideEffectScope` 7 值枚举（与 CLI 契约
     `ToolSideEffectScope` 同词表：none/workspace/git/network/system/session/userInteraction）；
     端到端证据：协议夹具的 started 帧从「靠滞后表放行」变为全量 parse 通过。
   - 回归（家族）：`packages/shared/test/mode-protocol-session-event-drift.test.ts`。
   - 锚点保留：常数 `KNOWN_V1_PAYLOAD_LAG`（`protocol-fixtures.test.mts`）已清空成**墓碑**，
     断言「被拒绝的键逐字等于表里的键」仍是长期绊网——未来任何被 schema 拒绝的会话事件
     夹具都会直接红；本节记录这张表为什么曾存在、后果是什么。
   Rust 镜像按**线上取值**建模（`execution_started_at: Option<f64>`、`read_only`、`side_effect_scope`），
   即镜像跟线上走、与 v1 schema 的滞后无关。
7. **协议夹具（Task 1）的规范化口径**：协议夹具住
   `packages/shared/test/fixtures/agent-protocol/`（与 §3.1 的行为夹具分居，见 ledger Ruling 3），
   录的是 stdio 的**整帧原样**，唯一例外是**机器相关路径前缀**换成占位符：
   `<data-root>`（隔离根）/ `<cwd>`（会话工作目录）/ `<tmp>`（系统临时目录）/ `<repo>`（本 checkout 根），
   词表与 `normalize.mts` 一致；单反斜杠与 JSON 转义的双反斜杠两种形态都换（同 §3 的路径规则）。
   理由：本机账户短名与 checkout 绝对路径既不属仓库、也不带来测试力量，而夹具的保真度要求的是
   形状与类型。规范化在录制器 `record-protocol-fixtures.mts` 里做（重录结果因此确定），
   并带一条「夹具里不得残留机器路径」的守卫。
8. **`originalBytes` / `returnedBytes` 不参与跨机器对拍**（2026-10-18）：这两个数按正文的
   UTF-8 字节计，而正文包含工作目录绝对路径（`Write`/`Edit`）或 shell 诊断前缀（`Bash`），
   其长度随机器与目录名变化。实测：`write-new` 的 10 字节差 = 该用例工作目录与 `<cwd>`
   的长度差；`bash-command-not-found` 的期望计数对应录制机的 shell 前缀。
   `tool_parity.rs` 因此只比 `truncated` / `budgetStrategy` / `display`，字节计数留给
   「同一隔离条件重录」的场景。**这不是产品差异**，不要在 Rust 侧为对齐数字改正文。

## 验收场景（Task 0）

1. `pnpm test:agent-runtime-oracle`（唯一入口；等价于
   `pnpm exec tsx --test packages/desktop/tests/agent-runtime-oracle/*.test.mts`）
   → 全绿。其中：
   - `driver.test.mts` → oracle 能起来、能吐 `turn.started` 与终态、能干净退出；
   - `isolation.test.mts` → 指向真实数据根（含旧根）直接抛错；隔离根下**归属文件与 CLI 布局
     都存在**；模型流量全部打到 mock provider；无解析失败行；
   - `repeatability.test.mts` → 同 prompt 两次运行，三层（工具调用 / 文件快照 / 事件流）一致；
   - `fixtures.test.mts` → 3 条夹具（只读一次成功 / 读不存在的文件 / 同 turn 连续两次工具调用）
     全部回放一致；夹具的 `expect` 由 oracle 录制；
   - `compare.test.mts` → 比较器对人为注入的差异报出差异（不能是恒真函数），
     且并行批次折叠不会吃掉工具输出差异。
2. 隔离哨兵可判伪：把 driver 的 `HOME`/`USERPROFILE` 覆盖改成别的目录，
   `isolation.test.mts` 必须**红**（spec §1.4 第 2 条）。
3. `cargo clippy --workspace --all-targets -- -D warnings` 零告警；`pnpm lint` 不高于基线。
4. 本套件**不进** `verify:pre-push` / release gate：它依赖机器环境（§6.4），
   接进 CI 是 Task 7 Step 2 的事。
