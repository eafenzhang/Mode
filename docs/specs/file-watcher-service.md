# Spec：file-watcher 频道（`crates/file-watcher`）

状态：2026-10-29 实现（W3-2 文件域的最后一块）。行为真源
`packages/services/src/fileWatcher/fileWatcherService.ts`。

## 方法

| 方法 | 语义 |
| --- | --- |
| `watch({path, recursive?})` | 建 watcher 并返回 `{id}`；`recursive` 缺省 false；失败报 `无法监视目录 '<path>': <原因>` |
| `unwatch({id})` | 关闭 watcher、清空订阅 |
| `disposeAll()` | 关闭全部 watcher（host 退出清理） |
| `onDynamicChange(id)` | 动态事件订阅，事件形状 `{dirPath, changedPath?}` |

## 防抖与 changedPath

- 每次底层事件把路径并入「本窗口集合」；无法归属到被监视目录（含目录自身）的事件记为
  「未知变更」。150 ms 内没有新事件时触发一次：
  - 恰好一个已知路径且没有未知变更 → `{dirPath, changedPath}`；
  - 其它情况 → 只发 `{dirPath}`（调用方保守刷新）。
- `dirPath` 是调用方传入的 `path` 原值；`changedPath` 是
  `path.resolve(dirPath, 文件名)` 形态（相对路径会按 cwd 补成绝对路径，与 Node 一致）。
- 多订阅者都会收到同一事件；`unwatch`/`disposeAll` 后不再触发。

## 错误与失效 id

- 底层 watcher 报错：先发一条 `{dirPath}` 最终事件，再在独立线程里清理实例
  （不能在通知回调线程里 drop 正在回调的 watcher）。
- 对已失效的 id 订阅：不抛到 RPC 链路，返回一个永不触发的空订阅，并输出一条告警
  （对应 TS 的 `Event.None`）。
- `watch` 失败会消耗一个 id（与 TS 先自增再尝试一致）。

## 已知差异

| 差异 | 说明 |
| --- | --- |
| 底层实现 | `notify 8`（Windows `ReadDirectoryChangesW` / Linux inotify / macOS FSEvents）替代 Node `fs.watch`；事件合并窗口与系统实现细节不同 |
| 事件类型 | TS 完全不看 `eventType`，这里同样忽略 notify 的 `EventKind`，只看路径 |
| 未知变更判定 | 目录自身/目录外路径记为未知；Node 在拿不到 filename 时同样省略 changedPath |
| 日志 | TS 的 `logger.warn` 换成壳侧 `shell_log`（经 `on_warn`） |
| 内存诊断 | 未移植 `registerMemoryDiagnosticsProvider`（Node 专属） |

## 验收

- `crates/file-watcher/tests/watcher.rs`（8 条，真实文件系统）：
  单文件变更带 changedPath、递归子目录、unwatch 停事件、disposeAll 停事件、
  多订阅者、缺失目录错误文案、失效订阅返回空 + 告警、id 单调。
- `crates/host-channels/tests/file_watcher_channel.rs`（5 条）：watch 返回 id、
  `onDynamicChange` 的 JSON 事件、失效 id 无事件、错误与未知命令、JSON 编码的 id。
- `cargo clippy -D warnings` 通过；频道名漂移守卫覆盖 `file-watcher`。
