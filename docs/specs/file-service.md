# Spec：file 频道（`crates/file-service`）

状态：2026-10-29 实现（W3-2 文件域，17 个方法）。行为真源：

- `packages/services/src/file/fileService.ts`
- `packages/services/src/file/workspaceFileIgnore.ts`（`.modeignore`）
- `packages/services/src/file/workspaceFileMentionFilter.ts`（默认过滤）
- `packages/shared/src/workspaceFileEntriesCodec.ts`（列式打包）
- `packages/shared/src/workspaceFileSearch.ts`（候选与打分）
- `packages/shared/src/protocol.ts`（FileEntry / FileStat / 读取结果类型）

## 方法

| 方法 | 关键语义 |
| --- | --- |
| `searchWorkspaceFiles` | `limit` 默认 1000、夹到 `[0,1000]`；`limit=0` 直接空；SQL 无 —— 走索引候选 + 模糊打分 |
| `readdir` | 默认隐藏 dotfiles；目录在前、再按名称排序；软链接目录跟随目标重新分类 |
| `stat` | 目录只返回 `{path,type}`；文件附 `size`/`mtimeMs` |
| `checkFilesExist` | 单批 ≤ 15；命中/未命中都缓存 60 s（LRU 100） |
| `resolvePath` | realpath；Windows 去掉 `\\?\` / `\\?\UNC\` 前缀 |
| `ensureConversationWorkspace` | `{dataRoot}/workspace/default`；`created` 表示本次是否新建 |
| `createDefaultWorkspace` / `createScratchWorkspace` | `{home}/ModeProject[/name]`；名称 trim、非空、不含 `/\` |
| `readTextFile` | 默认 128 KiB、上限 256 KiB；按 offset 读取；二进制判定后 `content=""` |
| `readFileRange` | 默认 256 KiB、上限 1 MiB；返回**顶层** `Uint8Array` |
| `readMediaPreview` | 默认 4 MiB、上限 8 MiB；超过上限报错；mediaType + base64 |
| `readBinaryPreview` | 默认/上限 25 MiB；base64 |
| `listWorkspaceFilesLength` | 打包串的 **UTF-16 码元数** |
| `listWorkspaceFilesRange` | 按 UTF-16 码元切块（512K 目标由调用方决定） |
| `readWorkspaceFileSearchIgnore` | 文件存在 → `source: "file"`；否则返回模板预览 `source: "template"` |
| `applyWorkspaceFileSearchIgnoreTransform` | `sync-gitignore` 只重写同步区；其它值按 `reset-defaults` 处理 |
| `writeWorkspaceFileSearchIgnore` | 原子写 `.modeignore` |

## `.modeignore`

- 新名 `.modeignore` 优先，旧名 `.zcodeignore` 兜底；新建一律用新名。
- 首次加载且文件不存在时自动创建：内容 = `.gitignore` 拷贝或内置模板，并写入双标记分区
  （同步区 / 默认排除区 / 自定义区）。
- gitignore 语义交给 `ignore` crate（与 TS `ignore` 包同源），**不手写解析**；匹配使用
  `matched_path_or_any_parents`，与 npm `ignores()` 的「父目录被排除后子路径同样忽略」一致。
- 读取/创建失败时 fail-open：内存里依次降级到 `.gitignore`、内置默认规则，并通过
  `on_warn` 输出一条告警；扫描不会因规则文件不可用而失败。
- 索引缓存 key = `workspaceIdentity ?? rootPath`；签名包含 rootPath 与 `.modeignore`
  的 `mtime:size`；TTL 60 s；最多 4 个 workspace。

## 索引与打包

- `.modeignore` 文件本身不进索引；隐藏目录不进候选但继续遍历。
- 并发 8 个 worker 共享目录队列；`EACCES/EPERM/ENOENT` 目录跳过，其它读错误中止。
- 排序：目录在前，再按名称（忽略大小写 → 原文 → 码点）。
- 列式格式：每行 `type\trelativePath`，`\`、`\t`、`\n` 转义；`name`/`path` 由解包方还原。

## 已知差异

| 差异 | 影响 |
| --- | --- |
| 排序用确定性比较器，不是 ICU `localeCompare` | 混合大小写/重音字符的相对顺序可能不同；搜索 top-K 不受影响（按分数+输入序） |
| UTF-16 切块遇到增补平面字符跨边界时整字符丢弃 | JS `slice` 会产生孤立代理项；路径方块拼接可能出现一个字符的偏差 |
| 非法 gitignore 规则 | 这里降级为空规则 + 告警；TS 在构建 matcher 时抛错 |
| 存在性缓存无 in-flight 合并 | 同一路径并发检查会重复 stat（结果一致，只多一点 IO） |
| `.modeignore` 原子写 | 复用 `atomic-file`（含目录锁、rename 重试），不显式 fsync；权限随 umask（TS 用 0644 + fsync） |
| 文件系统错误文案 | Node errno 文本 vs Rust `io::Error` 文本；只影响诊断，不是契约 |
| `listWorkspaceFilesLength` 的 RPC 编码 | 能放进 i32 用 `Int`，否则用 JSON 数字（两者在 JS 侧都是 number） |

## 验收

- `crates/file-service` 单元测试（20 条）：codec 转义/解包、Node `extname`、过滤规则、
  ignore 模板/去重/同步/重置、搜索打水分档、二进制判定、UTF-16 切片、verbatim 前缀。
- `tests/parity.rs`（8 条，`pnpm file:fixtures` 生成）：
  - 打包/解包（posix 与 Windows root）字节相等；
  - 5 组查询的 top-K 顺序相等；
  - 8 组默认过滤决策相等；
  - 9 组 `.modeignore` 匹配（含父目录排除、`!` 反选、anchored、`**`）相等；
  - 读取/加载来源（file / created-from-gitignore / created-from-template）相等；
  - sync/reset 3 组输出逐字节相等；无标记时的 transform 相等；
  - 小树整树扫描的 packed 串与 UTF-16 长度逐字节相等。
- `tests/service.rs`（9 条）：readdir 隐藏项与排序、stat、文本/二进制/偏移/越界、
  range、预览上限、工作区创建与名称校验、存在性批量与缓存、resolvePath、服务层搜索与忽略管理。
- `crates/host-channels/tests/file_channel.rs`（7 条）：对象参数、camelCase 字段、
  `readFileRange` 顶层二进制、长度 Int、分块裸字符串、ignore 读写、错误映射。
- `cargo clippy -D warnings` 通过；频道名漂移守卫覆盖 `file`。
