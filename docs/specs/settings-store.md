# Spec：settings 文件层（`crates/settings-store`）

状态：2026-10-29 实现（W1-1a / W1-1b / W1-1c）。行为真源：
`packages/services/src/setting/settingService.ts` 的 `resolveUserHomeDir` /
`getSettingsDir` / `readSettingsWithMeta` / `writeSettings` / `updateDataBaseDir` /
`ensureDefaultProject`。

## 路径

```
home = MODE_DESKTOP_HOME_DIR || HOME || USERPROFILE || 进程 home
dir  = {home}/.mode/v2          （MODE_DATA_ROOT_DIR_NAME = ".mode"）
file = {home}/.mode/v2/setting.json
```

三个环境变量都要 **trim 且非空** 才生效；空串等同于未设置。每次调用都按传入环境重新
解析（TS 踩过「模块加载时固化 homedir，测试只改 HOME 却读到真实用户目录」的坑）。

`HomeEnv` 的字段由调用方显式传入（`HomeEnv::from_process()` 读进程环境），这样测试不需要
改全局环境变量。

## 读取

| 情况 | 结果 |
| --- | --- |
| 文件不存在 | `Missing`（调用方用默认值） |
| JSON 合法 | `Loaded(value)` |
| 其它 IO 错误 | `Unreadable { error }`（TS 同样回落默认值，这里保留原因便于诊断） |
| JSON 非法 | 短重试 **3 次 × 300 ms**（每次**重新读文件**）；仍非法则把文件改名为 `setting.json.corrupt-<millis>` 并返回 `Corrupt { backup }` |

重试的意义：文件可能正被另一个写者覆盖，读方会短暂读到半截 JSON。只有连续失败才按坏文件
隔离，避免把用户配置误清成默认值。`ENOENT` 发生在隔离阶段说明别的读者已经处理过，属于
预期竞争，`backup` 为 `None`。

## 写入

`write_document(paths, &Value, &WriteOptions)` → `atomic_file::write_json_atomic`：
两空格缩进、无尾随换行、跨进程锁 + 临时文件 + rename 重试。目录不存在会自动创建。

## 模型层（W1-1b：默认值、合并、补丁）

默认值与「合并 + 补丁」的期望结果**由 TS 的 zod schema 生成**，Rust 不手抄字段表：

```bash
pnpm settings:fixtures     # → crates/settings-store/fixtures/settings.json
cargo test -p settings-store
```

黄金文件含三部分：`defaults`（`appSettingsSchema.parse({})`）、`allowedKeys`（schema 顶层键，
含没有默认值的可选字段，如 `dataBaseDir`）、`cases`（持久化值 + 补丁 → TS 期望结果）。

| 函数 | 语义 |
| --- | --- |
| `defaults()` | schema 给出的完整默认值 |
| `merge_persisted(v)` | 只保留 schema 允许的顶层键；对象逐层合并；缺失字段取默认值；**不去重 `recentProjects`**（与 TS 的读取一致） |
| `apply_patch(current, patch)` | 浅合并允许的键（未知键忽略，同 TS patch schema），再 `normalize` |
| `normalize(v)` | `merge_persisted` + `recentProjects` 去重并截断到 10（对应 `settingService.update` 的收尾） |

**与 TS 的两处差异**：

1. 字段类型不匹配时**只回落该字段**；TS 会让整个 `safeParse` 失败、整份文件回落默认值。
   逐字段回落更保守，不会因为一个坏字段丢掉整份配置。
2. 补丁**不做类型校验**：TS 的 patch schema 会让非法补丁整条 `update` 抛错；这里非法值
   在合并时回落默认值。需要严格语义的调用方应自行校验。

## 数据目录迁移与默认项目（W1-1c）

| 函数 | 语义 |
| --- | --- |
| `update_data_base_dir(paths, current_base, new_dir, windows, forbidden)` | `new_dir` trim 后为空 → 回落 `paths.home`；校验禁写目录 → 目标不同才复制 `{old}/.mode/v2` → `{new}/.mode/v2` → 把解析后的目标路径写入 `setting.json` 的 `dataBaseDir` |
| `copy_data_directory(old_base, new_base)` | 只复制 `.mode/v2`；**跳过** `setting.json` 与 `setting.json.*`（原子写中间态复制会 ENOENT）、跳过符号链接；**不覆盖**已存在的目标文件（对应 TS `force: false`） |
| `validate_data_base_dir_target(target, forbidden, windows)` | 仅 Windows 生效：目标等于或位于安装目录内 → `Forbidden` |
| `ensure_default_project(home)` | 在 home 下建 `ModeProject/`，返回 `{path, created}`；已存在时 `created=false` |

`SettingsService::update_data_base_dir` 的顺序与 TS 一致：**先复制、后写指针**；禁写时
不改 `setting.json`。已知差异见 `host-channel-adapters.md` 的 `setting` 小节。

## 不在本层（后续）

- **历史迁移**：`legacyAccountConnectionSettings`、`closeToTray`/`messageStreamShowReasoning`
  等一次性迁移以及 `sanitize*` 预处理；黄金样例刻意避开会触发迁移的输入。
- **写队列与 `expectedAccountSettings` 校验**：并发更新的先后判定。
- **setting 频道语义**：四个方法的接线已在 `crates/host-channels`，但写队列、账号 CAS 与
  补丁类型校验仍未补齐。

## 验收

`crates/settings-store/tests/settings_store.rs`（12 条）：路径优先级、空白回落、缺失、
合法加载、坏文件隔离（备份内容保留）、重试自愈（模拟另一写者写完）、原子写回形态、
`ensureDefaultProject` 幂等、数据目录复制跳过 `setting.json` 且不覆盖、Windows 安装目录
禁写、迁移后持久化指针、禁写时不写 `setting.json`。

`crates/settings-store/tests/document_parity.rs`（5 条）：默认值与顶层键对拍；6 组
「合并 + 补丁」与 TS 期望逐字段相等；未知顶层键丢弃而可选字段保留；类型不匹配逐字段回落；
`recentProjects` 去重截断。`cargo clippy -D warnings` 通过。
