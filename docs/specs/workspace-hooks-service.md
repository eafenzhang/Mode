# Spec：Workspace Hook 保存路径（Rust `workspace-hooks-service`）

状态：2026-10-30 保存侧（UI Hook 回写、原子写、已审核开关）与 TS 对拍通过；
**读侧（`loadHooks`）与频道注册尚未迁移**。

## 1. 范围

本 spec 固定 `crates/workspace-hooks-service` 的写侧行为：

1. `to_mode_hooks_events`：把 UI 的 `Hook[]` 折叠回配置的 `events` 结构；
2. `resolve_next_root_enabled`：根级 `enabled` 的归并；
3. `save_hooks`：按来源拆成用户级 / 工程级两份配置写回；
4. `atomic_write_json`：同目录临时文件 + fsync + rename；
5. `write_workspace_hook_configured_toggle`：把「已审核开关」写回同一条声明。

不在本 spec 内：`loadHooks` 与读侧投影（`fromProjectSnapshot` / `fromUserModeSource` /
`fromLegacyHooksConfig`）、trust store 读取、`grantWorkspaceHookTrust`、频道注册。

## 2. 行为真源

- `packages/services/src/hooks/workspaceHookSettingsModel.ts`（写侧函数）
- `packages/services/src/hooks/hooksService.ts`（`saveHooks` / `writeModeHooksConfig`）
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

## 9. 验收

```bash
pnpm hooks-save:fixtures
cmd //c "D:\tools\cargo-msvc.cmd test -p workspace-hooks-service"
```

fixture（`fixtures/hooks-save.json`）覆盖：

- 纯投影：`toModeHooksEvents` 2 组（含 matcher 合并）、`resolveNextRootEnabled` 3 组；
- 保存：3 组场景（多来源混合并保留其它顶层键、无文件时新建、已有 `enabled=true` 沿用），
  比较两份配置的完整 JSON；
- 已审核开关：5 组（关/开第二条/未知 id/磁盘声明被改/配置不可读），比较错误码+文案或写回后的文件。
