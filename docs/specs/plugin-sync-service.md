# Spec：插件归档同步（Rust `plugin-sync-service`）

状态：2026-11-01 首版（三个本地方法；归档与 marketplace source 待后续切片）。
对应 `packages/services/src/plugin-sync/pluginSyncService.ts` 与 `pluginSyncPath.ts`。

## 1. 范围

| Rust 方法                               | TS 方法                            | 说明                                                                                         |
| --------------------------------------- | ---------------------------------- | -------------------------------------------------------------------------------------------- |
| `list_local_user_plugin_candidates`     | `listLocalUserPluginCandidates`    | 读 `{home}/.mode/cli/config.json` 的 `plugins.dirs` / `plugins.enabledPlugins`，扫描内联插件 |
| `list_remote_user_plugin_statuses`      | `listRemoteUserPluginStatuses`     | 目标根 `{home}/.mode/plugins`；按目录名/pluginId 判重                                        |
| `check_remote_user_plugin_write_access` | `checkRemoteUserPluginWriteAccess` | 在 `{home}/.mode/plugins` 与 `{home}/.mode/cli` 写一次性标记                                 |

未迁移（本 crate 不提供）：`exportPluginsArchive`、`exportMarketplaceSourceArchive`、
`importPluginsArchive`、`importMarketplaceSourceArchive`。频道 `plugin-sync` 因此**暂不注册**。

## 2. 候选扫描口径

- manifest 查找顺序：`.mode-plugin/plugin.json` → `.zcode-plugin/plugin.json` →
  `.claude-plugin/plugin.json` → `.codex-plugin/plugin.json`；首个存在的文件解析失败即视为
  无 manifest（不再继续找后面的）。
- `name` 必须是非空 trim 字符串；`pluginId = "<name>@inline"`；`description/version`
  取 trim 后非空值。
- `componentTypes` 顺序固定 `skills → commands → hooks → mcp`；manifest 含对应键或目录
  存在（`skills/`、`commands/`、`hooks/hooks.json`、`.mcp.json`）即计入。
- `enabled`：`plugins.enabledPlugins[pluginId 小写]` 为布尔时覆盖，否则 `true`；
  `enabledOverride` 只在有布尔覆盖时出现。
- `id = sha256("<pluginId>:<canonicalRoot>")`（64 位小写 hex）。
- `directoryName`：目录 basename（空则 name）把 `\ / :` 替换为 `-`，再按安全相对路径校验；
  重复时追加 `-<sha256(canonicalRoot)[..8]>`。
- `sizeBytes`：递归 `symlink_metadata`，普通文件取 size，目录累加，其它类型 0
  （软链不跟随）。
- 同一 `realpath` 只处理一次；结果按 `name.localeCompare` 升序（与 TS 同一比较器）。

## 3. 远端判重

目标根固定 `{home}/.mode/plugins`。对每个请求：

1. `directoryName` 必须是安全相对路径；`pluginId` 非空且不含 `/`、`\`；
2. 目标目录存在 → `{exists:true, path, reason:"targetExists"}`；
3. 否则查 `plugins.dirs` 里已登记插件的 `pluginId`（小写比较，首个命中）→
   `{exists:true, path, reason:"samePluginId"}`；
4. 否则 `{exists:false}`。

## 4. 写权限

依次检查 `{home}/.mode/plugins` 与 `{home}/.mode/cli`：`create_dir_all` 后以
`create_new` 写 `.mode-sync-preflight-<pid>-<nanos>`，随后删除；任一失败立即返回
`{ok:false, path, error}`；全部成功返回 `{ok:true, path: "<dir1>, <dir2>"}`。

## 5. 已知差异

1. **配置 JSON 语法错误文案**：Node `JSON.parse` 的具体文案随版本变化；Rust 返回
   `invalid json object: <path>`。fixture 对两端都归一为 `<json-error>`，行为语义一致。
2. **目录去重后缀**：基于绝对路径的 sha256，沙盒路径不同会得到不同 8 位后缀；fixture
   把 `-<8 hex>` 归一并由 replay 校验其余字段。
3. **写权限标记名**：TS 用 uuid，Rust 用 pid + nanos；只影响一次性文件名。

## 6. 验收

- `pnpm plugin-sync:fixtures` 生成 `crates/plugin-sync-service/fixtures/plugin-sync.json`
  （3 个用例：完整 home、空 home、损坏配置）。
- `cargo test -p plugin-sync-service` 重建同构 home 并逐项深比较：
  `<root>` 路径、64 位 hash → `<sha256>`、去重后缀 → `<hash8>`；
  fixture 重复生成 sha256 一致。
