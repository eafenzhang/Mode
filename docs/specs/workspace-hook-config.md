# Spec：Workspace Hook 配置发现层（Rust `workspace-hook-config`）

状态：2026-10-30 补齐发现层并与 TS 对拍通过；**尚未接频道**（`hooks` 频道的 `grantWorkspaceHookTrust`
仍要等 W4 的 Agent authority）。

## 1. 范围

本 spec 固定 `crates/workspace-hook-config` 的行为：从 workspace 目录树发现 hook 配置文件、
校验 `hooks` 结构、产出 digest 输入（`SourceInput`），并把多份配置根的开关归并成运行时根。

不在本 spec 内：`saveHooks` 写入与 settings 模型（`hooksService` + `workspaceHookSettingsModel`）、
`grantWorkspaceHookTrust` 与 trust store 读取（后者已有 `crates/workspace-hook-trust`）、
摘要算法（已有 `crates/workspace-hook-digest`）、频道注册。

## 2. 行为真源

clean-room：只以本仓库 TS 为 oracle。

- `packages/shared/src/workspace-hook-config.ts`：schema、`discoverWorkspaceHookConfigPaths`、
  `createWorkspaceHookSourceInput`、`readWorkspaceHookProjectSources`、
  `resolveWorkspaceHookRuntimeRoot`、超时/输出上限/门禁解析。
- `docs/specs/clean-room-policy.md`：不读上游实现。

## 3. 公开面

| Rust | TS 对应 |
| --- | --- |
| `validate_hooks_config` | `workspaceHooksConfigSchema.parse` |
| `discover_workspace_hook_config_paths` | `discoverWorkspaceHookConfigPaths` |
| `create_workspace_hook_source_input` | `createWorkspaceHookSourceInput` |
| `read_workspace_hook_project_sources` | `readWorkspaceHookProjectSources` |
| `resolve_workspace_hook_runtime_root` | `resolveWorkspaceHookRuntimeRoot` |
| `resolve_timeout_ms` / `resolve_max_output_bytes` / `resolve_configured_gates` | 同名（2026-10-29 已落） |
| `WORKSPACE_HOOK_EVENT_NAMES` | 同名常量 |

## 4. schema 校验口径

- 根对象、`events` 对象、matcher 对象是 **strict**：出现未知键即整体非法。
  - 根只允许 `enabled` / `timeoutMs` / `maxOutputBytes` / `events`。
  - `events` 只允许 7 个事件名（`SessionStart`、`UserPromptSubmit`、`PreToolUse`、
    `PermissionRequest`、`PostToolUse`、`PostToolUseFailure`、`Stop`）。
  - matcher 只允许 `matcher` / `hooks`。
- 单条 hook 定义是 **passthrough**：未知字段原样保留，供 digest 与运行时消费。
- `type` 是判别字段：`process` 允许 `args`、`enabled`、`timeoutMs`、`statusMessage`；
  `command` 允许 `enabled`、`async`、`shell`（`true` 或非空字符串）、`timeout`、`timeoutMs`、
  `statusMessage`。两者的 `command` 都必须是非空字符串。
- 数值口径：`timeoutMs` / `timeout` / `maxOutputBytes` 必须是有限正数（0 与负数非法）。
- `hooks` 数组至少一条；`events` 的每个值必须是数组。
- 校验合法时输入原样可用（不做字段重排或补默认值）。

## 5. 发现顺序（digest 输入，不得重排）

1. 从 `workingDirectory` 向上找含 `.git`（目录或文件都算，worktree 的 `.git` 文件也命中）的根：
   - 找到 → 目录列表按「根 → cwd」顺序；
   - 到盘符根仍未找到 → **只扫 cwd 自身**（沿途祖先丢弃，与 TS 一致）。
2. 每个目录生成两个候选：`<dir>/mode.json`、`<dir>/.mode/config.json`，按目录顺序展开。
3. 只保留存在的候选（目录存在也算，读取阶段报 `io`）。
4. 显式 `explicitProjectConfigPath` 存在则追加到末尾，标记 `explicitProjectConfig: true`。
5. 按 `resolve(path)` 去重，保留首次出现（自动发现在前），不改变剩余条目相对顺序。

`discoveryOrder` 是上述列表的下标（包含被跳过/报错的条目）。任何重排都会让已有信任记录失效。

## 6. 读取与错误分类

逐条读取候选文件：

- 顶层不是对象，或没有 `hooks` 键 → **静默跳过**（既不算 source 也不算 error）。
- `hooks` 结构非法 → error，`category = "schema"`。
- JSON 语法错 → error，`category = "syntax"`。
- 文件读不出来（目录冒充文件、权限等）→ error，`category = "io"`。

`SourceInput` 字段：

| 字段 | 规则 |
| --- | --- |
| `canonicalPath` | `resolve(path)` |
| `baseDir` | 配置目录名是 `.mode` 时取上一级，否则取配置目录本身 |
| `configFileKind` | `explicit` / `mode.json` / `.mode/config.json` |
| `editable` | 非显式且 `canonicalPath === resolve(workingDirectory, ".mode/config.json")` |

## 7. 运行时根归并

按发现顺序遍历各根的 `{enabled?, timeoutMs?, maxOutputBytes?}`：

- `enabled`：任一根为 `true` 即 `true`（`false` 不会关掉前面已开的）。
- `timeoutMs` / `maxOutputBytes`：后出现的显式值覆盖前面的。
- 缺省 60 000 ms / 32 768 B；两者都按 `max(1, round(x))` 归一。

## 8. 已知差异与未纳入

- 未实现 `hooks` 频道的 `loadHooks` 组装（settings 模型、legacy `settings.json` 迁移、
  trust 标记）与 `saveHooks` 原子写；这些要等频道本体一起做。
- 未实现 `grantWorkspaceHookTrust`：必须转发给 Agent authority，阻塞在 W4。
- 路径按词法归一（折叠 `.`/`..`），不解析符号链接；与 Node `path.resolve` 的可见行为一致。
- TS 侧 `resolve` 对相对路径用进程 cwd；Rust 同样，但频道接线后应由调用方传绝对路径。

## 9. 验收

```bash
pnpm workspace-hook-config:fixtures
cmd //c "D:\tools\cargo-msvc.cmd test -p workspace-hook-config"
```

fixture（`crates/workspace-hook-config/fixtures/hooks-config.json`）覆盖 18 个目录树场景：

- 祖先链发现（`.git` 目录 / `.git` 文件 / 无标记时只扫 cwd）、`mode.json` 与
  `.mode/config.json` 两种候选、显式配置追加与去重；
- 读取错误三类（syntax / schema / io）与「无 hooks 键静默跳过」；
- schema 边界：`hooks: null`、负 `timeoutMs`、未知事件名、空 hooks 数组、非法 `type`、
  缺 `command`、`shell: false`、matcher 未知键、hook 未知字段 passthrough；
- 运行时根归并 8 组；`createWorkspaceHookSourceInput` 5 组（含 `.mode` 目录 baseDir、
  `mode.json` 类型、显式标记、editable 判定）。
