# Spec：settings 文件层（`crates/settings-store`）

状态：2026-10-29 实现（W1-1a）。行为真源：
`packages/services/src/setting/settingService.ts` 的 `resolveUserHomeDir` /
`getSettingsDir` / `readSettingsWithMeta` / `writeSettings`（仅文件层部分）。

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

## 不在本层（W1-1b 及其后）

- **schema 校验**：TS 在 zod 校验失败时返回默认值且**不隔离文件**。这需要 `AppSettings`
  字段表，属下一步。
- **默认值合并**：`setting.get()` 要返回「默认值 + 持久化值」的完整对象。
- **历史迁移**：`legacyAccountConnectionSettings` 等两处一次性迁移。
- **写队列与 `expectedAccountSettings` 校验**：并发更新的先后判定。
- **setting 频道**：`get` / `update` / `updateDataBaseDir` 等方法面与首个本地频道接线。

## 验收

`crates/settings-store/tests/settings_store.rs`（7 条）：路径优先级、空白回落、缺失、
合法加载、坏文件隔离（备份内容保留）、重试自愈（模拟另一写者写完）、原子写回形态。
`cargo clippy -D warnings` 通过。
