# Spec：插件归档同步（Rust `plugin-sync-service`）

状态：2026-11-21 更新（新增 §4d marketplace 安装/卸载编排与 config 写回；网络源待做）。
对应 `packages/services/src/plugin-sync/pluginSyncService.ts`、`pluginSyncPath.ts`，
以及 LT-15 P3 迁移的 `apps/mode-cli/packages/adapters/src/plugins/marketplace.ts`
与 `apps/mode-cli/packages/adapters/src/config/file-config.adapter.ts`。

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

## 4b. 归档格式与导入流程

- ustar + gzip；目录条目 typeflag `5`（路径带尾 `/`），文件 `0`；header 字段
  （mode/uid/gid/size/mtime/checksum、`ustar`/`00`、uname/gname=`mode`）与 TS 一致；
  子项按名称 localeCompare 排序；软链等非普通文件导出报错
  `unsupported plugin archive source: <path>`。
- 归档末尾追加 `.mode-plugin-sync.json`（2 空格缩进 + 尾随换行），内容
  `{plugins:[{name, pluginId, directoryName, enabled?}]}`。
- 导入：解到临时目录（`max_extracted_bytes = maxArchiveBytes`），读 metadata 并逐项校验
  `name/pluginId/directoryName` 非空与路径安全；manifest 的 pluginId（大小写不敏感）必须与
  metadata 一致；目标已存在 → `skipped(targetExists)`；同名 pluginId 已在 config →
  `skipped(samePluginId)`；否则复制到 `.importing-*` 后 rename，并原子写 config
  （追加绝对路径、按需写 enabledPlugins；写失败回滚目标目录）。单项失败记为
  `failed` + error，不影响其它插件。
- `overwrite: true` → `plugin sync overwrite is not supported`；归档体积超限 →
  `plugin sync archive exceeds limit: <actual>/<max>`。

## 4c. marketplace source 归档

- `source` 支持 `file`（读文件、sourceRoot=dirname）、`directory`（找
  `marketplace.json` 或 `.claude-plugin/marketplace.json`）与 `settings`（内联 manifest，无 sourceRoot）。
- manifest：`plugins` 支持数组或对象（对象键补成 name）；`dependencies` 过滤非空字符串；
  `pluginRoot` 取顶层或 `metadata.pluginRoot`。
- 闭包：DFS 去重；依赖 `name@marketplace` 只允许本 marketplace，跨市场直接报错；
  请求插件缺失报 `marketplace plugin not found: <name>`。
- 本地 source 解析：字符串 source 相对 pluginBase；对象 `directory` 取 `path`；
  远程 source kind（github/git/url/git-subdir/npm/pip）不镜像；fallback 为
  `pluginBase/<pluginName>`；镜像目录必须含 plugin manifest。
- 镜像 manifest：移除顶层 `pluginRoot`，有本地镜像时写 `metadata.pluginRoot = "plugins"`，
  每个插件 `source = "./<name>"`；目录名为 `<safe marketplaceId>-<sha256({manifestContent,
sourceFingerprints})[..12]>`。
- 归档只含 `<directoryName>/marketplace.json` 与镜像插件目录，metadata 是
  `{plugins: [], marketplaceSources: [{marketplaceId, directoryName}]}`。
- 导入校验 metadata 只有一个 marketplace、manifest.name 与 marketplaceId 一致；
  目标已存在 → `skipped`，否则递归复制 → `synced`；`overwrite` 直接拒绝。

## 4d. marketplace 安装/卸载编排与 config 写回（LT-15 P3）

真源为 TS bootstrap/plugins.ts 与 file-config.adapter.ts；全部写盘走 `atomic-file`
（两空格缩进 + 尾随换行，与 TS 字节对齐）。

### 4d.1 installed_plugins.json 记录层

- 读时归一：缺失/坏 JSON → 空状态；旧 map 形态（`plugins` 为对象，entry 或
  entry 数组）与数组形态都收敛为 `Vec<InstalledPluginRecord>`；缺 installPath、
  `plugins` 键缺失或非法的条目丢弃；scope 别名 `project`/`local` → `workspace`。
- id 归一：`@zcode-plugins-official` → `@mode-plugins-official`（读时归一，
  不改写用户数据原字段之外的部分）。
- 写回原子化；记录字段 camelCase 序列化，与 TS 同形。

### 4d.2 卸载编排 `uninstall_marketplace_plugin`

- 顺序：定位记录 → splice → 原子保存 →（`removeCache=true`）删 installPath 与
  用户 data 目录 → config 写回。
- `removeCache=false` 只摘记录不删文件（`keepData` 不参与该分支，与 TS 一致）；
  `keepData=true` 保留 data 目录。
- 记录不存在返回 `Ok(None)`（幂等）；config 写回失败传 `Err`（记录层已落盘，
  重试按记录不存在走幂等清理）。
- 路径安全：插件 id 白名单字符 `[A-Za-z0-9_.@-]` 以外替换为 `-`；官方市场改名后
  cache/data 目录旧 id 兜底（新目录优先，旧目录次之，都缺失落当前 id 目录）。

### 4d.3 安装编排 `install_marketplace_plugin`

- 顺序：读取 marketplace manifest → 依赖闭包（BFS；跨市场依赖默认拒绝，
  `allowCrossMarketplaceDependenciesOn` 显式放行；环检测报 `cycle`；缺失依赖报
  `Dependency not found`）→ 逐条拷贝到 cache 目录 → 记录 upsert → 原子保存 →
  config 写回（安装即默认启用）。
- 源：directory/local/裸字符串相对路径（含 `pluginRoot` 基准目录）+ filesystem/sea
  兜底；网络源 github/git/url/git-subdir 走 git clone 子进程（npm/pip/zip 显式不支持）。
- 网络源细节：`resolve_plugin_source_root` 返回 `ResolvedPluginSourceRoot { path,
  temp_dir }`，`cleanup()` 对应 TS `{path, cleanup}` 的所有权——网络 clone 落系统
  临时目录，复制进 cache 后必须清理；`MODE_GIT_BINARY` 覆盖 git 二进制；90s 超时；
  仅网络型错误（RPC failed/timeout/Connection reset 等）重试 3 次（间隔 1s×attempt，
  重试间重建目标目录）；无 sha 用 `--depth 1`，有 sha 补 `checkout`；`owner/repo`
  简写补成 GitHub HTTPS URL（`git-subdir`）。
- 已有记录重装：保留原 `installedAt`、覆盖其余字段；cache 目录先删后拷。
- 失败回滚本轮新建的 cache 目录（已有目录不动）。
- 返回 `MarketplaceInstallResult { dependency_closure, installed_plugins, enabled_ids }`；
  `enabled_ids` 是本次新置默认启用的 id（TS bootstrap 用它区分「新置默认启用」与
  「沿用既有显式配置」）。

### 4d.4 config 写回（`plugins` 段）

只操作 Mode 配置 JSON 的 `plugins` 段（`enabledPlugins`/`options`/`suppressedBuiltins`），
不碰 provider/模型等其他段；与 TS `file-config.adapter` 同口径：

- `enable_plugins_in_config`：把 `id → true` 写入 `plugins.enabledPlugins`；
  已有显式设置（含 `false`）的 id 跳过，不覆盖用户选择。
- `remove_plugin_from_config`：按别名集从 `enabledPlugins` 与 `options` 移除
  （别名 = canonical id + `mode-cua`/`computer-use` 名字别名 × 新旧官方市场段）。
- `add_suppressed_builtin`：归一后写入 `plugins.suppressedBuiltins` 数组，
  同时移除旧别名（去重幂等）。
- `remove_suppressed_builtin`：按别名集清除遗留 suppression（无变化不写盘）。
- 编排接线：安装成功后 `enable`；卸载记录路径成功后 `remove` + `remove_suppressed`；
  `config_path=None` 跳过写回（对拍测试/纯编排复用）。

## 5. 已知差异

1. **配置 JSON 语法错误文案**：Node `JSON.parse` 的具体文案随版本变化；Rust 返回
   `invalid json object: <path>`。fixture 对两端都归一为 `<json-error>`，行为语义一致。
2. **目录去重后缀**：基于绝对路径的 sha256，沙盒路径不同会得到不同 8 位后缀；fixture
   把 `-<8 hex>` 归一并由 replay 校验其余字段。
3. **写权限标记名**：TS 用 uuid，Rust 用 pid + nanos；只影响一次性文件名。
4. **gzip 字节**：Node zlib 与 flate2 压缩输出不同；tar 结构与条目顺序一致，双向可解。
   fixture 把 `archive` 归一为 `<archive>`，另存一份 TS 归档 base64 供 Rust 侧导入验证。
5. **元数据 mtime**：TS metadata 条目默认 `Date.now()`；fixture 固定时钟保证幂等，
   生产行为仍是当前时间。
6. **镜像 manifest key 顺序**：Rust 用 `serde_json::Map`（有序 map）序列化，键顺序与 TS
   对象插入顺序可能不同，导致 `directoryName` 的 12 位后缀不同；内容与导入行为一致，
   fixture 把 `-<12 hex>` 归一为 `<hash12>`。
7. **网络源传输**：TS 先走 GitHub archive 快路径（codeload tar.gz），网络型失败回退
   git clone；Rust 直接 git clone（同一解析结果，省一次 HTTP 探测）。zip/url-zip、
   npm、pip 在 TS 支持或显式报错，Rust 本切片统一返回
   `Plugin source is recognized but not supported in this runtime: <kind>`。
8. **git 子进程环境**：TS 用 `buildMarketplaceGitEnv`（sanitize + 显式恢复代理/CA）；
   Rust 直接继承父进程环境。manifest-only 大 catalog 的按需稀疏检出
   （`fetchMarketplacePluginFromRepo`）TS 有、Rust 待接（本地缺失即报错）。

## 6. 验收

- `pnpm plugin-sync:fixtures` 生成 `crates/plugin-sync-service/fixtures/plugin-sync.json`
  （3 个用例：完整 home、空 home、损坏配置）。
- `cargo test -p host-channels --test plugin_sync_channel`：候选/导出/导入往返、
  statuses、写权限与未知方法错误（2 条）。
- `cargo test -p plugin-sync-service` 重建同构 home 并逐项深比较：
  `<root>` 路径、64 位 hash → `<sha256>`、去重后缀 → `<hash8>`、marketplace 目录后缀 →
  `<hash12>`、归档字段归一；fixture 还保存插件与 marketplace 两份 **TS 生成归档**，
  由 Rust 侧直接导入验证跨语言 tar 兼容。fixture 重复生成 sha256 一致
  （文件 mtime 与 metadata 时钟固定）。
