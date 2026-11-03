# Spec：存储分类与占用聚合（Rust `storage-usage`）

状态：2026-11-02 第一版。
真源：`packages/services/src/storage/domain/storageCatalog.ts`、
`packages/services/src/storage/domain/usageAggregate.ts`、
`packages/shared/src/storage.ts`。

## 1. 范围

| Rust 入口                         | TS 对应                                  |
| --------------------------------- | ---------------------------------------- |
| `classify_storage_path`           | `classifyStoragePath`                    |
| `normalize_storage_relative_path` | `normalizeStorageRelativePath`           |
| `get_storage_category_cleanability` | `getStorageCategoryCleanability`       |
| `is_protected_storage_path`       | `isProtectedStoragePath`                 |
| `get_storage_clean_scopes`        | `getStorageCleanScopes`                  |
| `plan_storage_clean`              | `planStorageClean`                       |
| `StorageScanJob`                  | `createScanJob` 的状态/节流/终态部分     |
| `StorageUsageAccumulator`         | `createStorageUsageAccumulator`          |
| `group_storage_roots_by_volume`   | `groupStorageRootsByVolume`              |

`STORAGE_CATEGORY_IDS`、`StorageRootId`、`StorageCategoryId`、`StorageCleanability`、
`STORAGE_MORE_ENTRIES_PATH` 与 TS 常量/联合类型一一对应。输出对象字段顺序与 TS 一致；
所有字节/文件数为整数。

## 2. 分类语义

1. 路径先统一分隔符：`\` → `/`，去掉首尾 `/`；不解析 `.`/`..`。
2. 根级特例：
   - `rootId=home` 且启用自定义数据路径时，`v2` 整棵子树归 `other`，`entryKey=v2`；
   - `agent` 子树归 `other`，`entryKey=agent`。
3. 文件规则按声明顺序求值，先命中先生效：
   - subagent transcript（`cli/agents/<a>/<b>/transcript.jsonl`）聚合到前三段；
   - session SQLite、checkpoint pending/tmp、数据库/配置备份、coding-plan/bots 缓存、
     computer-use 日志、`v2/*.json`、`cli/config.json`、`agents/*.md`、`AGENTS.md`。
4. 前缀规则按前缀长度降序匹配（等长保持声明顺序），`entryKey` 取命中前缀的下一级。
5. 未命中时归 `other`，`entryKey` 取路径第一段（空路径为空串）。

清理范围只对可清理类别返回：先列递归范围（前缀规则 + `subagentTranscripts` 的
`cli/agents`），再列文件规则所在目录的浅层范围。`other` 和 `none` 类别恒为空。

## 3. 保护规则

任何类别下都拒绝删除以下 basename：`setting.json`、`setting.json.lock`、
`credentials.json`、`.credentials.json`、`.tokens`、`mode-stdio-tap.json`；
`v2/crash/live` 及其子树同样受保护。路径先按第 2 节归一。

## 4. 清理计划

`plan_storage_clean` 接收类别、候选文件、分类上下文、`now` 与固定本地偏移
`offset_secs`：

1. `cleanability=none` 的类别不删任何文件，全部候选计入 `skippedCount`。
2. 只保留分类与请求类别一致、且未被保护规则命中的候选。
3. `logs` 再过滤掉与 `now` 同一本地日期的文件；本地日按
   `(ms + offset) div 86400000` 比较（固定偏移，不处理夏令时跃迁）。
4. `subagentTranscripts` 按路径前三段作为会话 key，用**全部候选**（含非 transcript
   的 metadata/output）计算最近 mtime；只有 `now - latest > 24h` 的会话 transcript
   才进入目标。
5. 目标保持候选输入顺序；`skippedCount = 候选总数 - 目标数`。

## 5. 扫描 job 状态机

`StorageScanJob` 只处理快照与节流决策；异步 runner、AbortController 与定时器由适配层
驱动。快照初始为占位根（volume=null、bytes/fileCount=0、categories=[]）。

1. `progress`：更新 roots/errors。首次进度或距上次 emit ≥ `throttleMs` 时立即返回
   `Emitted`；否则在无 pending timer 时返回 `TimerScheduled { delay_ms }`，有 pending
   时忽略。
2. `timer_fired`：pending 时返回当前 scanning 快照并更新最近 emit 时间。
3. `finish_success`：写入最终 roots/errors；已 cancel 则 `cancelled`，否则 `complete`；
   写入 `finishedAt`，此后事件全部忽略。
4. `finish_error`：AbortError 或已 cancel 为 `cancelled` 且保留已有 errors；否则
   `failed` 并追加 `{path:"", code}`。
5. 终态不再触发 timer；调用方负责取消已排定的 timer。

## 6. 占用聚合

- 按分类累计 bytes/fileCount，并按 `entryKey` 聚合明细。
- 明细按 bytes 降序；并列时 `relativePath` 升序。超过
  `maxEntriesPerCategory`（默认 100）时，保留前 N 项，剩余项折叠成一条
  `relativePath="…"` 的汇总项。
- 快照固定输出全部 `STORAGE_CATEGORY_IDS`，无条目的类别补 0/空数组；
  volume 可为 `null`。
- 卷分组以 `dev:<deviceId>` 为 key；volume 缺失时用 `path:<rootPath>` 单独成组。
  组内根顺序保持输入顺序，组按 bytes 降序（并列保持首次出现顺序）。

## 7. storageService 编排核心（2026-11-04 追加）

真源：`packages/services/src/storage/app/storageService.ts`（app 层）。TS 的
runner/文件系统/卷探测是异步 IO，仍归宿主装配；本 crate 承载可同步化的编排：

### 7.1 job 身份门控（`StorageServiceState`）

- `start_scan` 先清 current（对旧 runner 的 abort 由宿主执行），生成 `scan-{n}`
  并同时成为 current 与 emit 门控基准（latestJobId）。
- emit 门控：`jobId === latestJobId` 才写入 latestSnapshot；被顶替的旧 job 尾包
  对 emitter 仍可见（TS fire 不受门控），但不得覆盖快照——对应 TS 注释
  「按最近一次启动的 job 判断」。
- `cancel_scan(jobId)` 只命中 current；`finish_job` 只在 current 仍是该 job 时
  清空；`clean`/`dispose` 前先 `cancel_current`（状态面）。

### 7.2 清理管线（`clean_category`）

顺序与 TS 一致：可清理判定（未知类别按 none 报错）→ 找根 → 作用域 → 列候选 →
计划 → 删除（keepDirectories = 作用域前缀全集）→ 合并 `skippedCount`。
错误文案与 TS 相同：`storage category is not cleanable: <id>`、
`storage root not available: <rootId>`；端口错误原样透传。
`offset_secs` 对应 TS 的系统本地时区（宿主注入）。

### 7.3 已知差异

TS app 层没有测试骨架；本节行为断言以 TS 源码语义为准，TS 完整对拍待补
（同 `terminal-service` 的处理方式）。异步 runner 接线（AbortController、
`done.then` 的 current 条件清理时序）由宿主用本模块 API 组合实现。

## 8. 已知差异

1. 明细并列排序：TS 使用 `String.prototype.localeCompare`，结果依赖 ICU/locale；
   Rust 使用确定性字节序。仅 bytes 相同的路径可能顺序不同，夹具只覆盖 ASCII
   小写路径以保持可比性。
2. TS 的 `$` 正则允许匹配末尾换行，Rust `regex` 只在输入末尾匹配；存储相对路径
   不含换行，夹具不覆盖该形态。
3. 本 crate 只做纯计算，不扫描文件系统、不读卷信息、不做清理 IO。
4. 清理计划的本地日使用调用方传入的固定偏移；TS 使用系统本地时区，跨夏令时边界
   的“同一天”判断可能不同（与 `automation-schedule` 的口径一致）。
5. 扫描进度在已有 pending timer 时跨过节流阈值：TS 立即 flush 但不会取消已排定
   timer，极端调度下可能再 flush 一次；Rust 状态机把 pending 置为失效，后续 timer
   回调不会产生重复快照。
6. 扫描 job 不拥有 runner/timer/AbortController；异步执行与取消信号由适配层驱动。

## 9. 验收

- `pnpm storage-usage:fixtures` →
  `crates/storage-usage/fixtures/storage-usage.json`
  - 97 条路径分类（全部类别、文件/前缀优先级、最长前缀、根级特例、Windows 分隔符、
    空路径、边界前缀）；
  - 14 条保护路径（全部保留 basename、保护前缀、近似路径、空路径）；
  - 11 个类别的 cleanability 与清理范围；
  - 3 组占用累加（分类聚合、折叠、tie 排序、默认/0/2 上限、null volume、
    自定义数据根特例）；
  - 2 组卷分组（空输入、同设备合并、无卷按路径分组、字节并列稳定顺序）；
  - 10 组清理计划（不可清理类别、日志同日/跨日、UTC/+8/-5 时区边界、
    subagent 活跃目录与恰好 24h 边界、空候选、备份/导出类别、自定义数据根）；
  - 8 组扫描 job 脚本（首次进度立即 emit、双窗口节流、无进度完成、错误码、
    cancel、AbortError、完成时清理 timer、失败追加错误）。
- `cargo test -p storage-usage`（1 条 TS 深比较回放 + 14 条 Rust 单测，
  其中 6 条为 §7 编排核心）。
- `cargo clippy -p storage-usage --all-targets -- -D warnings`。
