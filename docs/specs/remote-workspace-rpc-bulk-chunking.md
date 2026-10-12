# 远端工作区共享 RPC 通道的批量分块预算

状态：已拍板（2026-10-12）。关联：[remote-workspace-identity.md](./remote-workspace-identity.md)（远端服务归属）、[remote-workspace-bidirectional-attachments.md](./remote-workspace-bidirectional-attachments.md)（附件通道自身已按 512KB 分块）、[lan-remote-attach.md](./lan-remote-attach.md)（握手时序）。

## 背景与问题

远端工作区（LAN / SSH / WSL / Docker）的全部 RPC 共享一条连接：同一对端复用一条 WS（`windowRemoteConnectionRegistry` 按 target 复用），renderer 侧再经一条 MessagePort attachment 汇聚。传输层是单连接 FIFO——**大帧会把它之后的全部小帧挡在队列里（队头阻塞）**，交互 RPC（任务更新、readdir、git status、流式帧）的尾延迟等于在途大帧的传输时长。

本机工作区不受此影响：MessagePort 是进程内 structured clone，4MB 消息毫秒级完成，与网络带宽无关。

实测（环回，`createHttpServer` + `attachLanRemoteConnection`，真实分帧路径）：

| 场景                                      | 结果                                     |
| ----------------------------------------- | ---------------------------------------- |
| 空载小 RPC（system.info）                 | p50 0.21ms / p95 0.38ms                  |
| 4×4MB 批量传输进行中的小 RPC              | p50 0.52ms，**p95 = 97.87ms**            |
| 4MB 单帧（= 文件索引旧块大小）            | 环回约 20ms；真实 WiFi 按带宽线性放大    |

真实链路（WiFi / 跨网）下，单个 4MB 帧的线占用 = 4MB ÷ 带宽（10MB/s ≈ 400ms），期间所有交互 RPC 同步后移。文件索引单次拉取最多 ~25MB（37 万文件），旧实现按 **4MB/块**（`WORKSPACE_FILE_ENTRIES_CHUNK_SIZE = 4_000_000`）分块——是仓库内所有批量路径中唯一超过 1MB 的块（协议侧 `maxFrameBytes = 1MB`、`attachmentChunkMaxBytes = 512KB`；PDF/PPTX 256KB、附件上传 384KB、媒体预览 1MB）。表现为：远端工作区里打开文件树搜索 / Command Center 时整机交互发卡（键入丢帧 + 周边 RPC 尾延迟数百 ms）。

## 产品规则

1. **共享远端连接上的批量载荷必须分块，单块 ≤ 512 KiB（字符）。**
   - 口径与 `PROTOCOL_V4_LIMITS.attachmentChunkMaxBytes`（512 KiB 字节）对齐；文件索引块是路径为主的 ASCII 文本，512K 字符 ≈ 512KB 字节（CJK 路径最坏 3 字节/字符，上界 ~1.5MB，仍远低于旧 4MB→12MB 的上界）。
   - 单一实现点：`WORKSPACE_FILE_ENTRIES_CHUNK_SIZE`（`packages/shared/src/workspaceFileEntriesCodec.ts`）。任何新增批量走线路径（>1MB 的逻辑载荷）都必须按同口径分块，并在此 spec 登记。
2. **分块拉取序列不得改变语义**：`listWorkspaceFilesLength` + 按 offset 递增的 `listWorkspaceFilesRange`；块间让出事件循环（主线程每块只处理一小块，键入可插队）；拼接结果与原单条大消息逐字符相等。
3. **块大小是取舍而非越大越好**：RTT 开销 = 总字节 ÷ 块大小 × RTT；队头阻塞 = 块大小 ÷ 带宽。对端缓存（60s TTL）在每次 `listWorkspaceFilesRange` 上做一次命中校验（读 ignore 文件 + stat），块数增加只带来毫秒级常量，不改变渐进复杂度。
4. **不引入新通道/新协议**：仍是同一条连接的普通 RPC，只是把每帧变小。

## 状态所有者

| 状态                 | 唯一所有者                                              | 写路径                    |
| -------------------- | ------------------------------------------------------- | ------------------------- |
| 块大小常量           | `@mode/shared/workspaceFileEntriesCodec`                | 代码常量                  |
| 索引 packed 字符串   | 对端（服务方）FileService 实例（60s TTL / ignore 指纹） | `ensureWorkspaceFileIndex`|
| 拉取进度 / 拼装      | renderer `fetchWorkspaceFileEntriesPacked`              | 按 offset 顺序拉取        |

## 事件顺序

```text
renderer fetchWorkspaceFileEntriesPacked        对端 FileService
  listWorkspaceFilesLength ───────────────────►  ensureWorkspaceFileIndex（冷启动触发全量扫描；60s 内复用）
  ◄───────────── totalLength（字符数）
  loop offset = 0, 512K, …
    listWorkspaceFilesRange{offset, length} ──►  ensureWorkspaceFileIndex（命中缓存）→ packed.slice(offset, …)
    ◄───────────── chunk（≤512K 字符）
    packed += chunk；await setTimeout(0)  ← 让出事件循环：键入/渲染插队
  返回完整 packed（与单条大消息逐字符相等）
```

任一块请求失败 => 整体 fail-closed（调用方保留旧索引 / 显示错误），不返回残缺 packed。

## 验收场景

1. 不变式单测（`packages/shared/test`，CI 运行）：`WORKSPACE_FILE_ENTRIES_CHUNK_SIZE ≤ PROTOCOL_V4_LIMITS.attachmentChunkMaxBytes`；codec 往返（含转义与 Windows 路径回拼）逐项相等。
2. 分块循环单测（`packages/ui/test`）：以假 fileService 断言请求序列（首块 offset=0、步进=块大小、末块截断）与拼装结果；总量为 0 时不发 range 请求。
3. 手动（实机）：远端工作区 37 万文件仓库，打开文件树搜索/Command Center 拉取期间，键入与任务列表刷新不出现秒级冻结；对端日志文件不再出现单帧 4MB 级记录。
4. `pnpm typecheck`、`pnpm lint`、`pnpm --filter @mode/shared test`、`pnpm architecture:check --changed` 全绿。

## 不做 / 后续项

- **不做**：本 spec 只收敛块大小；不引入多连接/优先级调度（协议级多路复用属更大改造）。
- **后续项（需单独 spec）**：文件树搜索与 Command Center 目前走「全量索引 + 本地 Worker 打分」。对端已有同语义的 `fileService.searchWorkspaceFiles`（与 Worker 共用 `@mode/shared/workspaceFileSearch` 打分核心，@ 提及面板即走此路径，只回传命中项），切换后可完全消除这 ~25MB 的远端传输；属搜索架构变更，需先对齐。
