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

**未观察**：`installed_plugins.json` 在隔离数据根里的确切路径与一条完整记录样例；
本轮的路径结论来自类型与测试引用，不是运行时取证。

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

## 6. 复现

```bash
pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-plugin-host.mts
```

## 7. 协议常量（已存在，插件宿主必须复用）

- 官方 MCP 的 `_meta` 键：`com.mode/official-mcp-auth`（`packages/shared/src/official-mcp-auth.ts`，
  跨语言协议，改名即 breaking change）。
- `.mcp.json` 的 `auth.type` 唯一合法值：`mode_official`。
