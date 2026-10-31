# Spec：MCP 用户目录同步（Rust `mcp-sync-service`）

状态：2026-10-31 首版。对应 `packages/services/src/mcp-sync/mcpSyncService.ts` 的
**本地面** 7 个方法。

## 1. 范围

| Rust                                 | 真源                            | 说明                                           |
| ------------------------------------ | ------------------------------- | ---------------------------------------------- |
| `load_mcp_from_user_directory`       | `loadMcpFromUserDirectory`      | 读取 workspace（可选）+ user 的 native servers |
| `save_mcp_to_user_directory`         | `saveMcpToUserDirectory`        | `upsert` / `delete` / `set-enabled`            |
| `list_local_user_mcp_candidates`     | `listLocalUserMcpCandidates`    | 有效用户记录的候选投影                         |
| `list_remote_user_mcp_statuses`      | `listRemoteUserMcpStatuses`     | 按名字检查本地有效记录（无网络）               |
| `export_mcp_servers`                 | `exportMcpServers`              | 按候选 id 导出                                 |
| `check_remote_user_mcp_write_access` | `checkRemoteUserMcpWriteAccess` | 目录写标记探测                                 |
| `import_mcp_servers`                 | `importMcpServers`              | 跳过已存在、重写 filesystem 路径、写 Mode 配置 |

不在本 crate 内：`listWorkspaceMcpServerStatuses`——它是 `IModeAgentService.mcpList`
的投影，真实 connect/listTools 必须发生在 Agent 进程；等 W4 的 Agent 控制面就位后
再随频道一起接。因此 `mcp-sync` 频道**暂不注册**到壳内（不切半个频道）。

## 2. 目录与文件

| 来源     | user                           | workspace                | 服务器字段    |
| -------- | ------------------------------ | ------------------------ | ------------- |
| `mode`   | `<home>/.mode/cli/config.json` | `<ws>/.mode/config.json` | `mcp.servers` |
| `agents` | `<home>/.agents/mcp.json`      | `<ws>/.agents/mcp.json`  | `mcpServers`  |

- `home` 每次调用解析：`HOME.trim() || USERPROFILE.trim() || homedir()`（可注入覆盖，供测试）。
- workspace 缺失时报 `Missing workspace path for <source> workspace MCP config`
  （`source` 为 `mode`/`agents`）。
- 读取单文件用「mode 优先、为空再读 agents」：`readDirectoryServersFromPreferredSources`。
- native server 记录字段：`source: "zcodeagentmcp"`、`scope`、`name`、`config`、
  `enabled`（`config.enabled !== false`）、可选 `projectPath`、`location`
  （`{source, scope: user|project, directoryPath, projectPath?}`）、`file`
  （`{format: "json", filePath}`）。
- **顺序**：JSON 文档用 `IndexMap` 保留读入顺序；`load` 返回的数组顺序与文件一致
  （TS `Object.entries` 语义）。

## 3. JSON 读写

- 读：文件不存在 → `None`；读取失败 → `无法读取 MCP 配置文件 <path>: …`；
  解析失败 → `无法解析 MCP 配置文件 <path>: …`；根不是对象 →
  `MCP 配置文件 <path> 必须是 JSON 对象`。
- 写：两空格缩进 + 尾随换行，临时文件 `0600`（Windows 无等价位），原子 rename。
  顶层与 `mcp`/`mcpServers` 保留文档顺序；其余嵌套对象按 serde_json 字母序
  （对拍时 JSON 文件按键排序比较）。

## 4. 历史 `enable` 折叠

读 server map 时，对**仍含 `enable` 字段**的条目就地迁移（无论值真假）：
`disabled = enable === false || enabled === false`；写回只保留 `enabled`
（`true` 时删除该字段）。迁移后立即写盘；写盘失败不阻断读取（下次加载重试）。
不含 `enable` 的条目原样保留。

## 5. 写操作

- `upsert`：缺少 `config` 时报 `Missing MCP config for upsert action`；读取 mode
  现有 server map，插入/替换后写回。
- `delete`：从 mode server map 删除后写回。
- `set-enabled`：缺少 `enabled` 时报 `Missing enabled value for MCP set-enabled action`；
  定位由 `location ?? {mode, scope=user|project, projectPath}` 决定，未知来源报
  `Unsupported MCP settings directory source: <source>`；只修改已存在的对象型配置
  （`enabled=true` 删除字段，`false` 写 `enabled: false`），随后清理旧
  `mcp[directoryPath][name]` 覆盖。

## 6. 候选 / 状态 / 导出

- 有效用户记录：mode 记录非空则只用 mode，否则用 agents；按 `name` 升序。
- 候选 id：`sha256("<source>:<path>:<name>")` 的 64 位小写十六进制。
- 远程状态（本地检查）：名字键 `trim().toLowerCase()`，命中返回 `path`。
- 导出：按 id 找候选，缺失报 `mcp sync candidate not found: <id>`；
  结果带 `localHomeDir`。

## 7. 导入

- `overwrite: true` → `mcp sync overwrite is not supported`。
- 目标固定为 user mode 配置；已存在于目标或任一有效用户记录（同名键）→ `skipped`，
  并保留原记录路径；否则：
  - 先把 `enabled` 折叠进配置（`setServerEnabled`）；
  - filesystem MCP（名字为 `filesystem`/`file-system`/`fs`，或命令/参数含
    `@modelcontextprotocol/server-filesystem`/`mcp-server-filesystem`）且是 stdio 时，
    `args` 中的本地 workspace/home 绝对路径改写为远端对应路径（`remoteWorkspacePath` /
    `remoteHomeDir`）；远端路径按远端自身风格拼接（POSIX 或 Windows 盘符）。
  - 写入目标 map，记 `synced`；全部处理完后一次写盘。
- 结果：`{ results: [{name, status, path?, error?}] }`。

## 8. 已知差异

1. **JSON 键序**：顶层与 server map 保留文档顺序；`mcp`/`mcpServers` 之外的嵌套对象
   仍按字母序写出（TS 保留全部读入顺序）。对拍时配置文件按键排序比较。
2. **错误文本**：底层 IO 错误来自 Rust 运行时（如 `read failed` 兜底），与 Node errno
   文案不同；MCP 语义错误文案逐字对齐。
3. **写访问探测**：标记文件名使用进程号 + 纳秒（TS 用随机 UUID），探测语义一致。

## 9. 验收

- `pnpm mcp-sync:fixtures` 生成 `crates/mcp-sync-service/fixtures/mcp-sync.json`
  （8 个用例：用户读取、workspace 优先级、agents 回退、保存动作与错误、
  候选/导出、远程状态、写访问、导入与路径改写）。
- `cargo test -p mcp-sync-service` 回放同一份文件树与操作序列，逐项深比较结果与结束树
  （候选 id 归一为 `<sha256>`）。
