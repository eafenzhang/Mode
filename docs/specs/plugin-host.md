# 插件 / MCP / Skills 宿主（M3 生态域 1）

状态：2026-10-18 首版，**Task 0 的黑盒观察阶段**；未观察到的形状不写进实现。

出处：`packages/desktop/tests/agent-runtime-oracle/probe-plugin-host.mts`（默认隔离环境，
未安装任何额外插件）。纪律：只运行旧实现、观察请求体与事件，不读实现体。

## 1. 数据面（只读）

| 路径 / 类型 | 形状 | 所有者 | 本计划 |
| --- | --- | --- | --- |
| `<pluginRoot>/.mode-plugin/plugin.json` | `{ name, version, description, author: { name } }`（样本：`computer-use` 0.6.3） | 插件包自身 | 只读 |
| `installed_plugins.json`（marketplace root） | `InstalledPluginSummary[]`：`id` / `name` / `marketplace` / `enabled` / `scope` / `installPath` / `nativeScope` / `projectPath` / `installedAt` / `componentTypes` / `hookDetails`（`packages/shared/src/plugin-types.ts`） | Host services / 渲染层写 | 只读 |
| `.agents/plugins/marketplace.json` | Codex 市场约定（`packages/shared/src/plugin-marketplaces.ts`） | 市场安装流程 | 只读 |
| 内置官方 cache | 由 CLI seed；**不在** `installed_plugins.json` 里 | CLI seed | 只读 |
| 组件类型 | `agent` / `command` / `skill` / `hook` / `mcp` / `lsp` | 插件清单 | 只读 |

**黑盒取证（`probe-plugin-roots.mts` / `probe-plugin-fixture.mts`，2026-10-18）**：

- 插件存储根：`<dataRoot>/.mode/cli/plugins/`，内含 `cache/`、`data/`、`marketplaces/`。
- 官方 cache：`<插件存储根>/cache/mode-plugins-official/<name>/<version>/`；清单在
  `<version>/.mode-plugin/plugin.json`。
- `mode plugins list --json`（隔离 HOME）给出的逐项字段：
  `id` / `name` / `version` / `marketplace` / `source` / `enabled` / `rootPath` /
  `manifestPath` / `dataPath` / `declaredMcpServerNames` / `mcpServerNames` /
  `skillCount` / `skillRootCount` / `commandRootCount` / `hookDetails` / `diagnostics`。
- **官方 cache 由 CLI seed 覆盖**：手工写入的 fixture 插件在会话后消失，目录只剩
  `browser-use` / `computer-use` / `node-repl-host` 三个内置插件。⇒ 不能靠写 cache 目录
  安装插件；fixture 安装必须走 marketplace / 安装记录路径。
- **未观察**：`installed_plugins.json` 的一条完整记录样例，以及本地 marketplace 的安装命令
  （`plugins marketplace add <path>` / `plugins install` 的确切用法）。

## 2. 工具面（默认隔离环境）

默认声明 **21 条**工具，其中与插件/MCP/Skill 相关的是：

| 名字 | 类型 | input_schema 必填 |
| --- | --- | --- |
| `mcp__node_repl__js` | MCP 工具（`mcp__<server>__<tool>`） | `code`、`title`（可选 `timeout_ms`） |
| `Skill` | 内置 Skill 入口 | `skill`（可选 `args`） |
| `Agent` | 内置子代理入口 | `description`、`prompt`（可选 `subagent_type`、`run_in_background`） |

- MCP 工具的命名是 `mcp__<server>__<tool>`；**未声明的 MCP 名字不会动态路由**，
  调用直接得到 `tool_not_found`（见 §3）。
- `Skill` 的 `skill` 必须是「可用技能列表」里的名字；插件命名空间的技能用
  `plugin:skill` 的 fully qualified 形式（描述里逐字说明）。

## 3. 错误形状（逐字观察）

| 场景 | 结果 |
| --- | --- |
| `Skill {}`（缺 `skill`） | `tool_execution_failed` / `Tool input failed inputSchema validation` / code `TOOL_EXECUTION_FAILED` |
| `Skill { skill: "definitely-not-a-skill" }` | **`type:"Error"`** / `Skill not found: definitely-not-a-skill`（**没有** `code` / `detail`） |
| `mcp__nonexistent__tool` | `tool_not_found` / `Tool not found: mcp__nonexistent__tool` / code `TOOL_NOT_FOUND` |

注意 `Skill` 的「找不到技能」是 `type:"Error"`，不是 `tool_execution_failed`；
插件宿主的错误映射不能把它归到工具执行失败。

## 4. 权限

- 默认隔离环境下，`Skill`（非法/未知技能）与未知 MCP 工具调用**都没有**发
  `permission.requested`。
- **未观察**：会产生副作用的插件工具（例如 `mcp__node_repl__js` 执行代码）是否审批、
  `riskLevel` / `reason` 是什么。Task 0.4 未完成前，插件工具不接入 `permission::evaluate`
  的「按插件工具分档」逻辑。

## 5. 生命周期与负载

- 默认环境已经带 `mcp__node_repl__js`，说明插件/MCP 面在启动时就已装配；
  MCP 子进程启动约 5s 的基线见 `tauri-agent-runtime.md` §6.3。
- **未观察**：fixture 插件安装后的工具清单变化、启用/禁用开关的效果、MCP 服务器失败/
  断网/崩溃的形状、动态安装/卸载是否需要重启宿主。

## 5.1 资产清单与缓存（Task 1 落地，2026-10-18）

**清单**（新资产，CDN 上传时生成；不是旧格式）：

```json
{
  "schemaVersion": 1,
  "version": "0.1.0",
  "platform": "win32-x64",
  "entry": "plugin-host.mjs",
  "assetUrl": "https://.../plugin-host-0.1.0.mjs",
  "sha256": "<64 hex>"
}
```

- `schemaVersion` 必须为 `1`；新增字段必须升版本（客户端 `deny_unknown_fields`）。
- `entry` 必须是**单层文件名**（不允许 `/`、`\`、`..`）。
- 资产是**单文件 bundle**：Node 运行时随 Host 发布（DR-M3-2），这里不打包/解包 Node。
- 缓存布局：`<dataRoot>/plugin-host/<version>/<entry>`。
- 语义：缓存命中且 sha256 一致 → **不联网**；未命中 → 下载 → 校验 → 临时文件 + rename；
  sha256 不符或 HTTP 失败 → 报错且不落盘；断网 + 有效缓存 → 可用。
- 实现：`crates/plugin-host`（`PluginHostManifest` + `AssetCache`；5 条用例覆盖下载/命中/
  篡改/断网缓存/路径逃逸）。

## 5.2 sidecar 协议（Task 2 落地，2026-10-18）

一行一帧 NDJSON；方法集合封闭（未知方法 `-32601`）。实现：
`crates/plugin-host/src/protocol.rs`（Rust 严格解码）+ `tools/plugin-host/src/main.ts`（骨架）。

| 方法 | params | result |
| --- | --- | --- |
| `host/hello` | `{}` | `{ protocolVersion: 1, tools: <n> }` |
| `plugin/listTools` | `{}` | `{ tools: [{ name, description, inputSchema }] }` |
| `plugin/callTool` | `{ toolName, input }` | `{ content }` |
| `plugin/health` | `{}` | `{ ok: true, tools: <n> }` |
| `host/shutdown` | `{}` | `{}`（随后进程退出） |

- 帧形状：请求 `{id, method, params}`；应答 `{id, result}`；错误 `{id, error:{code,message}}`。
- 协议级错误的保留 id 是 `0`；正常请求 id 从 1 开始。
- 解析错误 `-32700`、非法请求 `-32600`、未知方法/工具 `-32601`。
- 同时带 `method` 与 `result`（或 `error`）的帧是**非法帧**，解码直接失败，不猜。
- 当前 sidecar 是骨架：`listTools` 只返回一个 fixture `echo` 工具，真实插件/MCP/Skills
  发现与执行在 Task 3–5；`callTool` 的 `echo` 只回显 `input.text`。
- 开发运行：`pnpm exec tsx tools/plugin-host/src/main.ts`（打包成 Task 1 的单文件 bundle
  是后续步骤；当前按源码运行）。

## 5.3 进程监督（Task 2 Step 3 落地，2026-10-18）

实现：`crates/plugin-host/src/supervisor.rs`。

- 子进程 stdin/stdout 走 NDJSON；stdout 由后台线程逐行解码，按 `id` 唤醒等待者。
- 请求超时只影响该请求，**不杀进程**；`plugin/callTool` 失败**不自动重发**（可能有副作用，
  重试是调用方的决定）。
- 子进程退出后，下一次请求按退避重启（初值/上限可配；成功一次后重置）。
- 每一代 reader 有自己的 `generation`：EOF 只让本代 pending 失败，不会误伤重启后的新请求；
  `reader_closed` 标记解决「EOF 早于 reap」的窗口，避免往已关闭的 stdin 写。
- `Drop` 时 kill + wait；写入失败会先失效当前子进程，下一次请求再 spawn。
- 3 条集成用例（`node -e` 假 sidecar）：正常往返、挂起超时后仍可用、崩溃后下一次请求重启成功。

## 5.4 工具注入（Task 3 落地，2026-10-18）

- `ToolSpec.name` 从 `&'static str` 改为 `String`：内置工具是常量，插件工具是运行时名字。
- `Session` 持有会话级 `extra_tools` + `plugin_tools`；`with_plugin_tools(source)` 构造时拉一次
  `listTools`。静态 `registry()` **不变**，查找顺序是「静态表 → 插件表」。
- `run_turn` 发给模型的工具名包含插件工具；`run_tool_batch` 对插件工具走
  `plugin/callTool` → `{content}` → `ToolOutcome`；RPC 错误映射为
  `tool_execution_failed` / `TOOL_EXECUTION_FAILED`。
- 未知名字仍是 `tool_not_found`；`listTools` 失败会把错误交给构造方（不静默降级）。
- **占位**：插件工具的 `family`/`read_only` 真实值未观察，当前登记为
  `TaskControl` + `read_only:false`，权限引擎因此判 Allow；Task 4 观察后收紧。
- 4 条用例：注入与调用、错误映射、未知名字、`listTools` 失败。

## 5.5 sidecar → 会话的适配（Task 4 Step 1 落地，2026-10-18）

实现：`crates/plugin-host-adapter`（`PluginHostSource`）。

- `list_tools`：`plugin/listTools` 的 `{tools:[{name,description,inputSchema}]}` →
  `agent_core::PluginToolDeclaration`（缺 `name` / `inputSchema` 直接报错，不静默补空）。
- `call_tool`：`plugin/callTool {toolName,input}` → sidecar 的 `{content}`；RPC 错误原样转成
  字符串交给会话层（会话再映射为 `tool_execution_failed`）。
- 权限不在适配器里判断：仍由 `permission::evaluate` 在会话层执行（当前插件工具的
  family/风险仍是占位值，Task 0.4 观察后收紧）。
- 端到端用例：真实 `node -e` sidecar → `SidecarSupervisor` → `PluginHostSource` →
  `Session::run_turn`，插件工具结果 `e2e` 正确回到会话事件流。
- 依赖方向：`plugin-host-adapter` 同时依赖 `agent-core` 与 `plugin-host`，两边都不反向依赖。

## 6. 复现

```bash
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-plugin-host.mts
```

## 7. 协议常量（已存在，插件宿主必须复用）

- 官方 MCP 的 `_meta` 键：`com.mode/official-mcp-auth`（`packages/shared/src/official-mcp-auth.ts`，
  跨语言协议，改名即 breaking change）。
- `.mcp.json` 的 `auth.type` 唯一合法值：`mode_official`。
