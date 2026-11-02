# Spec：数据根归属与备份（Rust `data-root`）

状态：2026-11-02 完成（归属文件、状态判定、迁移与初始化全部迁移）。
真源：`packages/shared/src/data-root.ts`、
`packages/services/src/data-root/ownership.ts`。

## 1. 范围

| Rust 入口                                   | TS 对应                                |
| ------------------------------------------- | -------------------------------------- |
| `parse_data_root_manifest`                  | `parseDataRootManifest`                |
| `is_accepted_data_root_product`             | `isAcceptedDataRootProduct`            |
| `resolve_data_root_dir`                     | `resolveDataRootDir`                   |
| `read_data_root_status`                     | `readDataRootStatus`                   |
| `write_data_root_manifest(_into_root)`      | `writeDataRootManifest(IntoRoot)`      |
| `forfeit_data_root_by_label`                | `forfeitDataRootByLabel`               |
| `forfeit_conflicting_data_root`             | `forfeitConflictingDataRoot`           |
| `read_legacy_data_base_dir_from_settings`   | `readLegacyDataBaseDirFromSettings`    |
| `discover_legacy_data_root_candidates`      | `discoverLegacyDataRootCandidates`     |
| `collect_legacy_candidate_stats`            | `collectLegacyCandidateStats`          |
| `cleanup_stale_migration_staging(_with)`    | `cleanupStaleMigrationStaging`         |
| `execute_data_root_copy_migration`          | `executeDataRootCopyMigration`         |
| `DataRootInitializer`（interactive/non-interactive/fresh/import） | `initializer.ts` 全部导出 |
| `resolve_data_root_action_from_env`         | `resolveDataRootActionFromEnv`         |

`initializer.ts` 与 `migration.ts` 已迁移；诊断根、pid、时钟、备份时间戳与迁移回调由调用方注入。

## 2. 归属文件

- 路径：`{base}/.mode/.mode-root.json`；读取时按 `.mode-root.json` →
  `.zcodium-root.json` → `.zcode-root.json` 探测，命中旧名只读不改写。
- 字段：`product`、`schemaVersion`（整数）、`createdBy`（desktop/cli/server）、
  `createdAt`、`firstSeenVersion`、可选 `migration{from,at,mode(copy|import)}`。
  非空字符串只看 trim 后是否为空，返回值保留原文；未知键忽略。
- 写入：结构体按 camelCase 序列化，两空格缩进 + 尾随换行；先写
  `<manifest>.<pid>.tmp` 再 rename，最后清理临时文件。
- product 互认：`dev.mode.app` 与历史 `dev.zcodium.app`。

## 3. 状态判定

| kind      | 条件                                                         |
| --------- | ------------------------------------------------------------ |
| `absent`  | `{base}/.mode` 不存在                                        |
| `unowned` | 无归属文件（`manifest-missing`）或 product 不匹配            |
| `corrupt` | 归属文件不可读/不可解析（`manifest-unreadable`）或 schema 高于支持版本 |
| `normal`  | 以上都通过，返回解析后的 manifest                            |

所有状态对象使用 camelCase 字段（`rootDir`），序列化形状与 TS 一致。

## 3.1 旧根探测、统计与清理

- 探测：`{base}/.zcodium` → `{base}/.zcode` 取第一个存在者为主候选；再从主候选
  `v2/setting.json` 的 `dataBaseDir`（支持 `~` 展开）解析自定义 base，取其历史根作为
  第二候选（一层、按 resolve 后路径去重）。
- 统计：递归统计条目数（目录/文件都计）、文件字节数（符号链接跳过）、最新 mtime；
  单项失败忽略，`modifiedAt` 用 UTC ISO（毫秒）。
- 清理：`{base}/.mode.migrating-*` 目录中 mtime 超过 1 小时的删除；非目录、
  符号链接与读取失败都跳过。mtime 通过注入回调以便测试。

## 3.2 复制迁移

`execute_data_root_copy_migration` 逐候选执行：

1. 统计所有候选总字节数（用于进度）。
2. 每个候选先检查取消；在 `{base}/.mode.migrating-<id>` 建 staging，复制树
   （跳过符号链接、非普通文件与 `.DS_Store`/`*.lock`/`setting.json.*` 临时文件；
   Unix 保留权限位）。
3. 在 staging 写归属（`createdAt`/`migration.at` 由注入时钟生成）。
4. 目标已合法 → 删除 staging 跳过；unowned/corrupt → 先备份让路；最后 rename 落位。
   rename 失败时若目标已变为合法则接受并发结果，否则失败。
5. 失败清理所有 staging；取消返回 `cancelled: true`。
6. 进度事件：`preparing`（每候选开始）→ 每个复制文件 `copying` → `finalizing`；
   `currentBaseDir`、累计 `copiedBytes` 与总 `totalBytes`。

staging id、时间戳、备份时间戳、进度与取消回调都由调用方注入。

## 3.3 初始化器

- 诊断模式：pending 时创建 `{tmp}/mode-startup/<pid>`（0700，Unix），清理超过 24h 的旧诊断根，
  并把数据根解析重定向到该目录；释放时同步清除重定向。
- 交互：`normal` → ready；`absent` 且无旧根 → fresh；`absent`+旧根 / unowned / corrupt →
  pending；隔离 base 的 unowned（manifest-missing）原地补写归属。
- 非交互：`normal` → ready；`fail` 抛错；`migrate` 有候选 → 复制迁移（失败/未合法化抛错）；
  其余按 fresh（隔离 base 且 action=fresh 时采用原地补写）。
- `execute_data_root_import`：先整体备份 `.mode` 为 `.mode.import-<ts>`，再以 `import`
  模式复制旧根；取消/失败返回带 `backupRoot` 的错误。
- 结果形状：`ready/initialized/pending`、`forfeitedRoot`、`backupRoot/migratedBases`、
  `MODE_DATA_ROOT_ACTION` 非法值回退 fresh。

## 4. 冲突备份

- `forfeit_data_root_by_label`：`{base}/.mode` 存在时整体 rename 为
  `{base}/.mode.<label>-<timestamp>`；重名依次加 `-1`…`-100`，超过 100 报错；
  目录不存在返回 `None`。
- `forfeit_conflicting_data_root`：只处理 `unowned`/`corrupt`，其它状态返回 `None`。
- 时间戳由调用方注入（TS 用本地时间 `yyyyMMdd-HHmmss`），避免 Rust 依赖本地时区。

## 5. 已知差异

1. 诊断根清理使用注入的 mtime/now（真实调用传系统时钟）；夹具不覆盖跨进程残留清理。
2. 备份时间戳格式由调用方决定；夹具用固定时间戳并在比较时归一化 `<ts>`。
3. `forfeit_conflicting_data_root` 对 `normal`/`absent` 返回 `None`（TS 的签名限定
   只接受冲突状态，运行时行为不在产品路径内）。
4. 复制迁移/初始化的 staging id、pid、tmpdir、时间戳由调用方注入；夹具对
   UUID/时间戳/备份后缀/诊断根路径归一化。

## 6. 验收

- `pnpm data-root:fixtures` → `crates/data-root/fixtures/data-root.json`
  （14 条 manifest 解析、3 条 product、7 条状态、1 条原子写、5 条备份、6 条旧 setting
  读取、7 条旧根探测、2 条统计、2 条 staging 清理、9 条复制迁移（fresh/import、
  unowned/normal 冲突、取消、无候选、多候选、临时文件、嵌套目录）、6 条 action 解析、
  17 条初始化器（交互 pending/fresh、非交互 fail/migrate/fresh、隔离 adopt、import/取消））。
- `cargo test -p data-root`（真实临时目录逐项比较；路径归一化 `<root>`、时间戳 `<ts>`）。
- `cargo clippy -p data-root --all-targets -- -D warnings`。
