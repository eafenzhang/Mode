# Spec：Workspace Hook 读写路径（Rust `workspace-hooks-service`）

状态：2026-10-30 保存侧与读侧（`loadHooks`）均与 TS 对拍通过；**频道注册尚未迁移**。

## 1. 范围

本 spec 固定 `crates/workspace-hooks-service` 的行为：

写侧：

1. `to_mode_hooks_events`：把 UI 的 `Hook[]` 折叠回配置的 `events` 结构；
2. `resolve_next_root_enabled`：根级 `enabled` 的归并；
3. `save_hooks`：按来源拆成用户级 / 工程级两份配置写回；
4. `atomic_write_json`：同目录临时文件 + fsync + rename；
5. `write_workspace_hook_configured_toggle`：把「已审核开关」写回同一条声明。

读侧：

6. `from_project_snapshot` / `from_user_mode_source` / `from_legacy_hooks_config`：
   三类来源投影成 UI 的 `Hook[]`；
7. `load_hooks`：按固定来源顺序合并投影，附带快照、`hooksEnabled` 与 trust store 状态。

不在本 spec 内：`grantWorkspaceHookTrust` 转 Agent authority、频道注册。

## 2. 行为真源

- `packages/services/src/hooks/workspaceHookSettingsModel.ts`（写侧函数与读侧投影）
- `packages/services/src/hooks/hooksService.ts`（`saveHooks` / `writeModeHooksConfig` / `loadHooks`）
- `packages/shared/src/workspace-hook-mutation.ts`（原子写与已审核开关）

## 3. 配置路径

| 作用域 | 路径 |
| --- | --- |
| 用户级 | `<home>/.mode/cli/config.json` |
| 工程级 | `<workspace>/.mode/config.json` |

`home_dir` 由调用方传入（壳内取用户主目录），不做环境变量回落——可测试性优先。

## 4. UI Hook → 配置（`to_mode_hooks_events`）

- 按 `event` 分组；同一事件内按 `matcher` 相等复用同一 matcher 条目（缺省 matcher 与缺省合并），
  新 matcher 只写 `matcher`（存在时）与 `hooks` 数组。
- 单条声明（`get_writable_hook`）：
  - `custom` 对象**展开到顶层**（不是 UI Hook 的未知顶层键），随后 `type` / `command` / `enabled` 覆盖同名键；
  - `enabled` 取「声明级开关」：有 `configuredState` 时，若 UI `enabled === configuredState.configuredEnabled`
    写 `declarationEnabled`，否则写 UI 当前值；
  - `statusMessage` 仅非空时写入；
  - `command`：`async === true` 才写、`shell` 为真值才写、`timeout` 为真值才写；
  - `process`：`args` 非空才写、`timeout` 为真值时写 `timeoutMs = timeout × 1000`。

## 5. 根级 `enabled`（`resolve_next_root_enabled`）

任一 Hook 满足「`enabled` 为真且（无 `configuredState` 或 `enabled !== configuredEnabled`）」→ `true`；
否则沿用文件里已有的值（缺省则不写这个键）。

## 6. 保存（`save_hooks`）

1. 用户级：`editable !== false` 且（无 `location` 或 `location` 为 `mode`/`user`）。
2. 工程级：`editable !== false` 且 `location` 为 `mode`/`project`，并且（无 `configuredState`
   或 `configuredState.sourcePath` 解析后等于当前工程配置路径）——避免把别的 workspace 的
   声明写进本工程。
3. 两次 `writeModeHooksConfig`：读现有 JSON（不存在/不可解析按 `{}`），保留其余顶层键，
   只替换 `hooks.enabled`（按 §5）与 `hooks.events`（按 §4），原子写回。

## 7. 原子写（`atomic_write_json`）

同目录临时文件 `.名称.<pid>.<纳秒>.<序号>.tmp` → `create_new` 写入 `JSON.stringify(value, null, 2) + "\n"`
→ `sync_all` → `rename`；失败时删除临时文件。临时文件必须与目标同目录，跨盘 rename 不原子。

**已知差异**：serde_json 的对象键按字典序输出，TS 保持插入顺序；文件语义等价、文本不同。
壳内首次写回会让既有文件的键顺序规范化。

## 8. 已审核开关（`write_workspace_hook_configured_toggle`）

1. 目标校验：条目 `editable`、来源 `editable`、来源类型是 `.mode/config.json`、来源
   `canonicalPath` 解析后等于目标路径，任一不满足 → `workspace_hooks_snapshot_mismatch`。
2. 读配置：读失败或 JSON 不合法 → `workspace_hooks_config_unreadable`。
3. 结构校验：根不是对象、`hooks` 不合 schema、`events`/matcher/声明缺失 → 各自
   `workspace_hooks_snapshot_mismatch`，文案固定。
4. 摘要守卫：用**审核时的** `resolvedTimeoutMs` / `resolvedMaxOutputBytes` 重算声明摘要，
   与 `hookDeclarationDigest` 不等 → 「declaration changed after review」。
5. 写入 `enabled` 后原子写回。

已知边界（与 TS 相同）：只改根级 `hooks.timeoutMs` / `hooks.maxOutputBytes` 时，
回填审核值会让摘要仍相等，因此该守卫捕获不到 root 默认值变化；执行侧用当前快照评估，
不存在提权风险。

## 9. 读侧投影

每个来源的每条声明投影成一个 UI `Hook`，字段规则与写侧互逆：

- `configuredState` 恒由条目位组成（`sourceRootEnabled` / `declarationEnabled` /
  `runtimeHooksEnabled` / `configuredEnabled` + `sourcePath`，路径统一正斜杠）；
- `enabled` 取 `configuredEnabled`（**不是**声明自身的 `enabled`）；
- `type` / `command` / `matcher` / `statusMessage` 按条目原值输出，缺省键不出现；
- `process` 类型输出 `args`（缺省 `[]`）；`command` 类型才输出 `async` 与 `shell`
  （`shell` 为 `true` / 字符串命令 / `false`，`unset` 不出现）；
- `timeout`：`command` 类型优先取声明里的秒级 `timeout`；否则由 `timeoutMs` 换算，
  且必须先取整再除：`round(ms) / 1000`（2500ms → 2.5，不是 3）。两个来源都没有时不写该键；
- `custom`：声明里除 `type` / `command` / `args` / `async` / `enabled` / `shell` /
  `statusMessage` / `timeout` / `timeoutMs` 之外的顶层键，整组输出；为空不写该键；
- `location`：工程来源 `{source:"mode", scope:"project", directoryPath:<来源目录>,
  projectPath:<工作区>}`；用户 mode 来源 `{source:"mode", scope:"user",
  directoryPath:<home>/.mode/cli}`；legacy 来源 `{source:"agents"|"claude",
  scope:"project"|"user", directoryPath:<base>/.agents|.claude, projectPath?}`。

三类来源的差异：

| 来源 | `id` | `editable` | `workspaceHook` |
| --- | --- | --- | --- |
| 工程快照 | `reviewItemId` | 来源 `editable` | 有：`reviewItemId` / `sourceFileIndex` / `workspaceIdentity` / `bundleDigest` / `hookDeclarationDigest` / `trustState` / 快照条目原字段 |
| 用户 mode | `hook-mode-user-{下标}` | 来源 `editable` | 无 |
| legacy | `hook-{source}-{scope}-{序号}` | 恒 `false` | 无 |

工程来源的 `trustState`：摘要命中持久信任 → `trusted_persistent`，否则 `pending_trust`。

legacy 来源额外约束：只认 `WORKSPACE_HOOK_EVENT_NAMES` 内的事件；`type` 必须是
`command` / `process` 且 `command` 非空，否则整条跳过；`enabled` 恒 `false`；
`timeout` 由 `timeout` 或 `round(timeoutMs / 1000)` 得到（与 mode 来源的取整顺序不同），
缺省不写键；`args` / `async` / `shell` / `statusMessage` 有才复制。

## 10. `load_hooks`

1. 固定来源顺序，逐段追加：
   工程快照 → `agents`(project) → `claude`(project) → mode user → `agents`(user) →
   `claude`(user)。
2. 快照只由**工程来源**构建（用户级 mode 配置不进 bundle），`workspaceIdentity` 与
   `projectPath` 都用工作区路径的正斜杠形式。
3. `hooksEnabled = hooks.some(hook => hook.enabled === true)`，空列表为 `false`。
4. 返回 `{hooks, hooksEnabled, snapshot, trustStoreCorrupt}`；`snapshot` 无工程来源时为 `null`。

## 11. trust store 读取

路径：`<storage.dir>/security/workspace-hook-trust-v1.json`。`storage.dir` 取自用户配置
`storage.dir`：`~/` 展开到 home，绝对路径原样，相对路径相对 home；缺省 `<home>/.mode`。

- 文件不存在（ENOENT）→ 空集 + `trustStoreCorrupt=false`（视为「还没建过」，不是损坏）；
- 其它 IO 错误或解析/schema 失败 → 空集 + `trustStoreCorrupt=true`；
- 命中条件：记录的 `workspaceIdentity` 与当前工作区身份**逐字相等**（原生分隔符）。

## 12. 验收

```bash
pnpm hooks-save:fixtures
pnpm hooks-load:fixtures
cmd //c "D:\tools\cargo-msvc.cmd test -p workspace-hooks-service"
```

fixture（`fixtures/hooks-save.json`）覆盖：

- 纯投影：`toModeHooksEvents` 2 组（含 matcher 合并）、`resolveNextRootEnabled` 3 组；
- 保存：3 组场景（多来源混合并保留其它顶层键、无文件时新建、已有 `enabled=true` 沿用），
  比较两份配置的完整 JSON；
- 已审核开关：5 组（关/开第二条/未知 id/磁盘声明被改/配置不可读），比较错误码+文案或写回后的文件。

fixture（`fixtures/hooks-load.json`）覆盖 5 组：混合来源（含 legacy 取整与非法条目跳过）、
已持久信任、trust store 损坏、用户级配置不合法、空工作区。比较 `hooks` 全量字段、
`hooksEnabled`、`snapshot` 与 `trustStoreCorrupt`；`snapshot.discoveredAt` 与来源
`sourceFiles` 分隔符是已知的文本差异，比较前归一。
