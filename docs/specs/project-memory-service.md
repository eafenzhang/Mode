# Spec：Project Memory 只读服务（`crates/project-memory`）

状态：2026-10-29 实现。行为真源：`packages/services/src/memory/memoryService.ts` 与
`projectMemoryStableRead.ts`。本 crate 是 `memory` 频道的**完整**实现（两个方法全覆盖），
因此它是 M4b 里第一个可以接管所有权、接上路由开关的频道。

## 为什么选它做第一个本地频道

- 只有两个方法，且**全部只读**，不存在跨进程写与并发合并；
- 不承载活性探测（渲染层的半开管道心跳走 `host.ping`，因此 `host` 频道不能先搬）；
- 数据面自包含：只读 `{dataRoot}/cli/memories/projects`，不依赖 provider/网络/凭据。

`setting` 频道有四个方法（含 `updateDataBaseDir` 要拷贝整个数据目录）且承担写路径，
留在后续波次。

## 目录契约

```
{dataRoot}/cli/memories/projects/<workspaceId>/memory/<file>
```

- `dataRoot` 由调用方传入（Host 的 `resolveDataRoot` 结果 = `{dataBaseDir}/.mode`）。
- 只认普通目录与普通文件；**任何符号链接都被跳过或拒绝**（目录与文件两层都查）。
- 文件名只允许 `MEMORY.md` 或 `*.md`；读取时必须**大小写精确**命中目录项——
  大小写不敏感的文件系统否则会让别名绕过目录白名单。

## 方法

| 方法 | 返回 |
| --- | --- |
| `listProjectMemories()` | `[{id,label,updatedAt,files:[{name,path,kind,size,updatedAt}]}]` |
| `readProjectMemoryFile({workspaceId,fileName})` | `{content, updatedAt}` |

- `kind`：`MEMORY.md` 为 `index`，其余为 `item`；排序为 index 优先，其次文件名。
- `label`：`workspaceId` 形如 `<slug>-<16 位十六进制>`（大小写不敏感）时取 `slug`（trim 后
  非空），否则用原 id。
- workspace 按 `updatedAt` 降序、同值按 `id` 升序。
- `updatedAt` 是毫秒时间戳（文件 mtime）。
- 根目录不存在 → 空数组；根存在但不是普通目录 → 错误。

## 读取的安全与稳定性

1. 校验 `workspaceId` / `fileName` 是单段路径且文件名合法；
2. `projects` / `<workspace>` / `memory` 三级都必须是普通目录（非符号链接）；
3. 目录项名必须与请求名逐字符相等；
4. `canonicalize` 后文件必须落在 `projects` 根内（防 symlink 逃逸）；
5. 打开前后比较文件快照；读完后再次比较。快照不一致 → `PROJECT_MEMORY_FILE_CHANGED`；
6. 正文上限 **5 MiB**，超限 → `PROJECT_MEMORY_PREVIEW_LIMIT_EXCEEDED`。

**与 TS 的差异（Windows）**：TS 用 `dev/ino/mtimeNs/ctimeNs` 判定同一快照；Rust std 在
Windows 上拿不到稳定的文件索引，退化为 `size + mtime`，并在 Unix 上额外比较 `dev/ino`。
同一时间戳精度内的同尺寸改写理论上可能漏判（Windows 侧）。

另有一处 TS 行为未复制：TS 在「大小写别名命中」时先 `lstat` 再报错；这里直接以
`NotFound` 报错（目录项没有精确命中）。

## 验收

`crates/project-memory/tests/project_memory.rs`（7 条，Windows 下 6 条 + Unix 专属 1 条）：
空根、列表结构与排序、label 解析、读取内容与时间戳、非法路径段、大小写精确、5 MiB 上限、
符号链接跳过（Unix）。`cargo clippy -D warnings` 通过。
