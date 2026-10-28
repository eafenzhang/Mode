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
- **`mcp__node_repl__js`（有副作用的 MCP 工具，`probe-plugin-permission.mts`）**：
  - `permission.requested`：`riskLevel:"high"`、reason `High risk tools require explicit approval`；
  - `started`：`readOnly:false` / `sideEffectScope:"system"`；
  - 结果：`display.kind:"mcp_tool"`、`budgetStrategy:"artifact"`。
- **插件 MCP（fixture，`mcp__plugin_fixture_fixture__echo`）**：`riskLevel:"medium"`、
  reason `Tool has side effects and requires approval`、options 三条；
  `started` 是 `readOnly:false` / `sideEffectScope:"network"`；
  结果 `budgetStrategy:"truncate"`，`display.kind:"mcp_tool"`。
- Rust 实现：`ToolSpec.plugin` 携带 `PluginToolMetadata { riskLevel, sideEffectScope,
  budgetStrategy }`；声明缺省取 **medium / network / truncate**（插件 MCP），
  可由 sidecar 声明覆盖（内置 node_repl 显式 high/system/artifact）。权限 Ask 用声明的
  risk；`started` 用声明的 scope；结果预算用声明策略。
- **未观察**：`allow_project` 对插件工具的 `ruleContent` 形状，因此暂不把 `Ruleset`
  用于插件工具；只读 MCP 工具是否也 medium/是否不问，也未观察。

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
| `plugin/refresh` | `{}` | `{ servers: <n> }`（杀掉现有 MCP 子进程并重扫 cache） |
| `plugin/listTools` | `{ refresh?: boolean }` | `{ tools: [{ name, description, inputSchema, riskLevel?, sideEffectScope?, budgetStrategy? }] }` |
| `plugin/callTool` | `{ toolName, input }` | `{ content }` |
| `plugin/health` | `{}` | `{ ok: true, tools: <n> }` |
| `host/shutdown` | `{}` | `{}`（随后进程退出） |

- 帧形状：请求 `{id, method, params}`；应答 `{id, result}`；错误 `{id, error:{code,message}}`。
- 协议级错误的保留 id 是 `0`；正常请求 id 从 1 开始。
- 解析错误 `-32700`、非法请求 `-32600`、未知方法/工具 `-32601`。
- 同时带 `method` 与 `result`（或 `error`）的帧是**非法帧**，解码直接失败，不猜。
- **真实发现（2026-10-18）**：sidecar 扫描
  `<dataRoot>/.mode/cli/plugins/cache/<marketplace>/<plugin>/<version>/`，读
  `.mode-plugin/plugin.json` 与 `.mcp.json` 的 `mcpServers`；按需 spawn MCP server
  （initialize → tools/list → tools/call）。工具名 `mcp__plugin_<plugin>_<server>__<tool>`。
- 工具声明附带可选元数据 `riskLevel` / `sideEffectScope` / `budgetStrategy`（插件 MCP 默认
  medium/network/truncate；内置 node-repl-host 跳过，由内置面提供）。
- 坏 server：`tools/list` 失败 → 该 server 的工具**静默缺失**；server 崩溃后下一次调用
  重新 spawn（与 oracle 行为一致）。
- **运行中安装/卸载（Task 6，2026-10-18）**：`plugin/refresh` 会先 kill 现有 MCP 子进程
  （它们的 cwd 是插件目录，不先杀会在 Windows 上 EBUSY），再重扫 cache；adapter 的
  `list_tools` 每次先发 refresh。`probe-plugin-install-uninstall.mts` 验证：
  安装前 `[]` → 安装+refresh 后工具出现并可调用 → 卸载+refresh 后 `[]`。
- **启用/禁用过滤（2026-10-18）**：启用态在 `<dataRoot>/.mode/cli/config.json` 的
  `plugins.enabledPlugins["<name>@<marketplace>"]`（`installed_plugins.json` **没有**
  enabled 字段）。sidecar 每次 discovery 读取它：显式 `false` 跳过、显式 `true` 启用、
  无记录时默认启用（官方默认关闭的 `computer-use` 除外）；`node-repl-host` 由内置面提供，跳过。
- **未做**：sidecar 崩溃的 Session 级 E2E（supervisor/adapter 单测已覆盖重启）。
- 开发运行：`pnpm exec tsx tools/plugin-host/src/main.ts`（打包成 Task 1 的单文件 bundle
  是后续步骤；当前按源码运行）。复现：`probe-plugin-sidecar.mts`。

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

## 5.6 fixture 安装与 Skill 发现（Task 0.2，2026-10-18）

实现/复现：`probe-plugin-installed.mts`（自包含：建 marketplace → CLI 安装 → 同数据根跑 oracle）。

- 本地 marketplace 清单（Codex 约定，被 `plugins validate` 接受）：

```json
{
  "name": "fixture-marketplace",
  "owner": { "name": "Fixture" },
  "plugins": [{ "name": "fixture", "source": "./plugins/fixture", "description": "Fixture plugin" }]
}
```

- 安装命令：`plugins marketplace add <dir> --scope user` → `plugins install fixture@fixture-marketplace --scope user`。
- 落点：`<plugin storage>/cache/<marketplace>/<name>/<version>/`；`plugins list --json` 的
  `rootPath` / `manifestPath` / `dataPath` 指向这里。
- **Skill 布局**：`skills/<name>/SKILL.md`（frontmatter `name` + `description`）。
- 调用 `Skill { skill: "fixture:fixture-skill" }` 与 `{ skill: "fixture-skill" }` **都能命中**；
  结果是 `<skill_content name="…">…Base directory for this skill: <cache 路径>…</skill_content>`，
  `budgetStrategy:"truncate"`。
- **MCP fixture（`probe-plugin-mcp.mts`）**：`.mcp.json` 的
  `{ mcpServers: { fixture: { command, args } } }` 被接受；`plugins list` 显示
  `declaredMcpServerNames:["fixture"]`、`mcpServerNames:["plugin:fixture:fixture"]`。
- 工具名规范：**`mcp__plugin_<plugin>_<server>__<tool>`**（本 fixture 是
  `mcp__plugin_fixture_fixture__echo`）；工具总数 21 → 22。调用成功，正文 `hello from mcp`。
- **未观察**：MCP 启动耗时、失败/断网/崩溃形状；非插件（全局）MCP 服务器的命名与风险。

## 5.7 MCP 生命周期（Task 5 Step 3，2026-10-18）

实现/复现：`probe-plugin-mcp-lifecycle.mts`（fixture 有 `echo` + `crash`；另有 `broken`
插件的 command 指向不存在的文件）。

- 启动：`startupMs ≈ 5468`（CLI + 两个 MCP 插件 + 内置 node_repl）；工具总数 23。
- **坏 server 降级**：插件仍在 `plugins list` 里（`mcpServerNames:["plugin:broken:broken"]`），
  `diagnostics` 为空；工具清单**不含**它的工具（静默缺失，不是启动失败）。
- **崩溃**：调用 `crash` 让 server 退出 → 工具错误 `type:"SdkError"`、`message:"Connection closed"`、
  `code:"CONNECTION_CLOSED"`；**下一次 `echo` 调用自动重启 server 并成功**（约 138ms）。
- **未观察**：反复崩溃/重启上限、启动超时、并行会话下的 server 复用。
- **Rust 侧已对齐（2026-10-18）**：`PluginToolError` 区分 `SdkError`（message
  `Connection closed`、code `CONNECTION_CLOSED`）与 `tool_execution_failed`；
  supervisor 的 `Closed` 或含 `closed` 的 RPC 错误映射为前者。下一次 refresh/listTools 会
  重启 sidecar（adapter 用例覆盖：崩溃 → SdkError → 重启后可列工具）。

## 5.9 sidecar bundle（2026-10-18）

- 构建：`pnpm run build:plugin-host`（`scripts/build-plugin-host-bundle.ts`，esbuild 单文件
  ESM）→ `dist/plugin-host/plugin-host.mjs`（约 9.1KB）+ `manifest.json`。
- manifest 形状与 `crates/plugin-host/src/manifest.rs` 的 `PluginHostManifest` 一致；
  `assetUrl` 默认是相对文件名 `plugin-host.mjs`，上传 CDN 时用
  `MODE_PLUGIN_HOST_ASSET_URL` 覆盖；`sha256` 按 bundle 字节计算。
- bundle 可直接用 `node` 跑；`probe-plugin-sidecar.mts` 支持
  `MODE_PLUGIN_HOST_BUNDLE=<path>` 验证 bundle 形态（hello/listTools/callTool 全通过）。
- **bundle 校验（2026-10-18 落地）**：`buildPluginHostBundle()` 可注入输出目录/entry/
  version/platform/assetUrl，供测试复用。校验契约：
  1. `manifest.schemaVersion=1`，`entry` 指向的文件存在且非空；
  2. `sha256` 等于 bundle 字节的 sha256；
  3. `assetUrl` 默认 `plugin-host.mjs`，环境变量/参数覆盖生效；
  4. 以 `MODE_DATA_BASE_DIR=<空目录>` 启动 bundle，`plugin/health` 返回
     `{ok:true,servers:0}`，`plugin/listTools` 返回 `{tools:[]}`；
  5. 用例：`packages/desktop/tests/plugin-host-bundle.test.mts`（随 CI 的 desktop 测试跑）。
- **未做**：CDN 上传与真实 `assetUrl` 的部署、JS bundle 的 Node 版本矩阵。

## 5.10 Hook 来源布局（2026-10-28）

- 插件 hook 文件按以下优先级发现：
  1. `hooks/hooks.json`（Claude 约定）；
  2. 根目录 `hooks.json`（Codex 插件约定，仅在标准路径不存在时启用）；
  3. `manifest.hooks` 声明的路径或内联对象。
- 标准路径存在时不读取根 `hooks.json`，避免同一插件同时带两套布局时重复注册 hook；
  根 `hooks.json` 与 `manifest.hooks` 指向同一文件时按 realpath 去重。
- 根 `hooks.json` 与标准路径一样使用 `{ "hooks": { ... } }` 包装结构；缺少 `hooks`
  对象时按 `plugin_hook_invalid` 报错，不静默跳过。
- 事件名仍只允许 `HookEventName` 闭集；未知事件产生 `plugin_hook_unsupported_event`
  警告，不阻塞插件其余组件。
- 验收：`apps/mode-cli/packages/adapters/test/plugin-hook-sources-compat.test.mts`
  覆盖根 `hooks.json`、标准路径优先、manifest 路径去重与缺失 `hooks` 包装。

## 6. 复现

```bash
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-plugin-host.mts
```

## 7. 协议常量（已存在，插件宿主必须复用）

- 官方 MCP 的 `_meta` 键：`com.mode/official-mcp-auth`（`packages/shared/src/official-mcp-auth.ts`，
  跨语言协议，改名即 breaking change）。
- `.mcp.json` 的 `auth.type` 唯一合法值：`mode_official`。
