# 远端工作区对话双向附件

## 产品规则

### 范围

- 覆盖桌面窗口发起的**全部远端连接类型**：SSH / WSL / Docker / LAN。
- 「双向」= ① 本地 → 远端发送附件（含 LAN 解禁）；② 远端 → 本地回读历史附件并下载落盘（含 agent 产出的 artifact 行）；③ 从远端工作区文件系统浏览选文件拉回对话。
- 不含手机远控（`web-remote-replayable`）专项缺口；其链路因收向授权放开自然受益，但不作为本 spec 验收对象。

### 状态所有者

| 状态                  | 所有者                                                    | 说明                                                                      |
| --------------------- | --------------------------------------------------------- | ------------------------------------------------------------------------- |
| composer 附件上传状态 | Renderer `composerAttachmentUploadStore`                  | scope 粒度，纯 UI 态                                                      |
| 跨机暂存事务          | 窗口 Host `IPromptAttachmentTransferService`              | 唯一跨机转换点：SSH/WSL/Docker 走 `backend.upload`，LAN 走附件 chunk 通道 |
| 附件字节              | 远端机器（staging 路径）或远端会话 artifact store         | 均由远端 CLI host 持有                                                    |
| 引用权威              | 远端 CLI 投影中的 `userInput.attachments` / `artifact` 行 | 回读授权、编辑重发以此为准                                                |

### 不变量

1. **主机本地路径永不抵达远端 agent**：所有路径在 host 边界转换（stage / materialize），失败 fail-closed（`staged:false` 拒发，维持现状语义）。
2. **回读只信权威行**：renderer 自报的 ref 必须精确匹配远端 CLI 投影中的行；跨 session、任意路径读取一律拒绝。放开 MIME 限制不松动此边界。
3. **renderer 不感知传输差异**：LAN 与 SSH 对 renderer 是同一个 `stage` 契约，传输差异只存在于 host 连接句柄内部。

### 发向：LAN stage 解禁

- LAN 句柄（`packages/desktop/src/host/index.ts` 的 `createWindowLanRemoteConnectionHandle`）中对 `stage` 与 `materializePromptAttachments` 的无差别抛错 stub 替换为真实现，composer 零改动。
- `stage()`：host 侧流式读本地文件（`fs.createReadStream` 分块 + 流式 SHA-256，不整读进内存）→ 经 `connection.services.modeAgentService.attachmentBeginV4/ChunkV4/CommitV4` 推入对端 CLI 会话 artifact store → 返回 `{ref, bytes, staged:true}`。
- 进度复用句柄既有 `progressEmitter` → `onDynamicProgress`；断点续传依赖 `begin` 的 `nextChunkIndex` 幂等语义，事务 TTL 5 分钟内可续。
- `adopt` = no-op（artifact store 对未引用条目按 `attachmentUnreferencedTtlMs` 24h 自动回收）；`cancel` = `attachmentAbortV4` 终止在途事务；`cleanup` = 未提交则 abort，已提交交给 TTL GC。
- **限额（维持现状，已确认不做调整）**：LAN 单文件上限 = `PROTOCOL_V4_LIMITS.attachmentMaxBytes`（20 MiB）。stage 发起前先 `stat` 本地文件，超限不发起事务，报「局域网连接附件单文件上限 20 MiB，请改用 SSH/WSL/Docker 工作区，或先把文件放到对端工作区」。SSH 流式上传无此上限，此不对称为已知取舍。
- 对端 CLI 无 `putSessionAttachment` hook → `fault.attachment.putUnsupported` → 报「对端版本不支持附件上传」，不静默降级。
- SSH/WSL/Docker 的 staging（`remotePromptAttachments.ts`）行为不动。

### 发向：LAN materialize guard 替换

- `dataBase64` / `textContent` 附件 → 放行（内联字节随命令直达对端 CLI，与手机远控同路径）。
- `localPath` 附件 → host 侧读文件转内联 `dataBase64` 后放行；**内联上限 = `attachmentMaxBytes`（20 MiB，与上行一致）**，超限可读错误拒发。载体只能是内联：`ModePromptAttachment` 是 strict union（仅 `dataBase64`/`localPath`/`textContent`），无法携带 artifact ref；20 MiB 内联是旧 `attachment/put` 既已验证可承载的量级。保证不变量 1。
- **materialize 对 `localPath` 的排序判定**（LAN 与 SSH 共用语义，防 ref↔prompt 遗留转换丢失来源信息）：
  1. 路径在 staging 根（`isRemotePromptAttachmentPath`）→ 放行（现状）；
  2. 路径在**本机**存在 → 按本地文件处理（SSH 上传改写 / LAN 内联转换）；
  3. 本机不存在、**目标机**存在（SSH：`exec test`；LAN：经远端 `fileService` 探测）→ 判为已在目标机，原样放行（不读、不转，路径本就是目标机的，不变量 1 不破）；
  4. 两侧都不存在 → 可读错误拒发。
  - 已知局限：两机存在**同名不同内容**的路径时，规则 2 判本机优先（与现状上传语义一致）。彻底消歧需协议层 origin 标记，见「不做」。

### 发向：远端选文件（反向拉取）

- composer「+」菜单新增「从工作区选择文件」入口，仅在 `isRemoteAttachmentTarget` 时显示；本地工作区仍走系统原生选择器，行为不变。
- 对话框 = `DirectoryBrowser` 增加 files 模式（现状只列目录），底层 `fileService.readdir` 已经是远端代理，无新协议。
- 选中项在 **composer 层**标记 `origin: "workspace-pick"` + 远端绝对路径：跳过 stage（文件已在目标机）→ 构造 `AttachmentRef{ref: 远端路径}` → 主发送路径（v4 `sendText` refs，`SessionPane` 直发）原样送达远端 agent，零拷贝读取。
- **origin 不入协议**：`AttachmentRef` 为 strict 结构；若遗留路径（`sendPrompt`/`enqueueTaskCommand` 的 `ModePromptAttachment`）经 ref↔prompt 转换把该路径落成 `localPath`，由上节 materialize 排序判定兜底（主路径不经过 materialize，不依赖此兜底）。
- 只能选文件不能选目录；不设大小限制（无字节传输）。
- **选择窗口必须屏幕居中**：非嵌入形态的 `DirectoryBrowser` 用 `fixed inset-0 + items-center` 居中，但它被 composer 挂在 `ConversationBottomDockTransition` 的 `motion.div` 下，该元素带内联 `transform` 与 `will-change-transform`——transform 会成为 `fixed` 后代的包含块，弹窗于是只在底部输入栏那一小条盒子里居中，视觉上贴着屏幕下方溢出。因此非嵌入 overlay 必须 `createPortal` 到 `document.body`，让 `fixed` 以视口为包含块；`embedded` 形态（远端建连选路径）维持原位不动。

### 收向：回读授权与下载

- **授权与媒体判定分离**（`apps/mode-cli/packages/bootstrap/src/mode-protocol-v4/v4-gateway.ts`）：
  - `attachmentRead` 的 resolver 改为按权威行授权：`userInput` 行内 `ref/previewRef` 精确匹配即授权，**不看 MIME**（现状 `isPreviewable` 只认 image/video/pdf + `readAttachmentPayload` 的 `previewNotMedia` 双重卡死非媒体）。
  - `attachmentPreviewSource` 保留媒体 gate 不动（非媒体无预览源语义）。
  - **读响应 wire schema 不设媒体白名单**（`packages/shared/src/mode-protocol-v4/transport.ts` 的 `v4AttachmentReadResultSchema`）：客户端对读块响应的校验只保留结构与容量语义。读通路（`attachmentRead`）预览与下载共用，媒体白名单挂在这条响应上会把非媒体回读（txt/office/`application/octet-stream`）打成客户端响应校验错误——网关已按权威行授权放行，客户端再按「预览口径」拒绝等于双重卡死。媒体 gate 只属于 `attachmentPreviewSource`：其 params 终审 `mediaOnly:true`，`local_path` 分支另有 video-only refine。
  - **容量不改**：非视频 20 MiB（`attachmentMaxBytes`）、视频 30 MiB（`attachmentPreviewMaxBytes`），超限 → 稳定码 `previewTooLarge`。
- **artifact 行授权**：resolver 增加 `kind === "artifact"` 行的 ref 精确匹配授权（session 归属由 publisher 保证）；读端复用既有 `mode-artifact://` → `readAttachment` 通路。
- **下载 UI（全部接入 `platform.saveFile`，复用 image-preview-dialog 的 blob→saveFile / `<a download>` 回退模式）**：
  1. 文件 pill（`UserInputAttachmentList` 非媒体）：新增「下载」；
  2. `ChatMediaAttachmentPreviewDialog`（视频/PDF）：新增下载按钮；
  3. `ImagePreviewDialog`：已有下载，不动；
  4. `ArtifactRowView`：新增下载按钮，媒体 mimeType 同时提供预览入口。
- 拉取走既有 `context.readAttachment`（`attachmentReadV4` 分段请求，gateway 整读缓存后切片，无新协议）；远端会话经既有 `modeAgentService` 代理（LAN/SSH 方法面已含），零新增代理面。
- 下载只在字节完整后触发 `saveFile`，不落半截文件。

### 错误处理（fail-closed，全部可读文案）

发向：

| 场景                                       | 行为                                                                                                                             |
| ------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------- |
| LAN 单文件 >20 MiB                         | `stat` 先于事务校验，不发起上传，报「局域网连接附件单文件上限 20 MiB，请改用 SSH/WSL/Docker 工作区，或先把文件放到对端工作区」   |
| 对端无 `putSessionAttachment`              | `fault.attachment.putUnsupported` → 「对端版本不支持附件上传」，不静默降级                                                       |
| LAN 传输中断                               | stage 抛错 → `failed`（可恢复失败）；连接/会话恢复后自动重传，见「失败恢复」；5 分钟 TTL 内 `begin` 按 `nextChunkIndex` 幂等续传 |
| `staged:false`                             | 维持现状 `RemoteAttachmentNotStagedError` 拒发                                                                                   |
| LAN materialize localPath >20 MiB 内联上限 | 可读错误拒发（不变量 1 优先）                                                                                                    |
| materialize 排序判定两侧都不存在           | 可读错误拒发                                                                                                                     |
| SSH 上传失败                               | 现状不动（「远端附件上传失败：<文件名>」+ 尽力回收）                                                                             |
| 会话未就绪                                 | 现状 `waitingSession` 排队                                                                                                       |

### 失败恢复（远端断链 / 运行时换代）

- 状态所有者不变：renderer `composerAttachmentUploadStore`；上传控制器、自动重试定时器由当前 composer owner 持有。
- **瞬时错误重试**：按 500ms → 1.5s → 3s 有界退避自动重试（覆盖远端重连、agent 冷启动与 cold-session-resume 的常见窗口）；用尽后仍失败才进入 `failed`。
- **可恢复失败** = `uploadStatus:"failed" && uploadErrorKind:"transient"`（连接关闭、网络中断、会话暂不可用等非确定性错误）。`permanent`（超限、格式非法、对端不支持）与 `runtimeRestarted`（换代重传配额用尽）不自动恢复，避免重复失败或无限循环。
- **恢复触发点**：`attachmentSessionId` 变为非空、`remoteSessionId` 变化、远端 service proxy 代际变化（同一 `remoteSessionId` 换代时 id 不变，必须显式作为触发点）、runtime 重新 available。触发时把可恢复失败与 `waitingSession` 项统一重置为 `queued`、清零 `autoRetryCount`，保留 `runtimeRebuildRetryCount`（换代配额仍受限），由唯一上传队列重传。
- **owner 卸载**：`V4PaneConversationProvider` 在远端断链进入 remote-waiting 时会卸载 composer。旧 owner 必须中止在飞上传、清除重试/完成定时器、移除本 scope 队列项，并把 `queued|uploading|committing` 降为 `waitingSession`；下一个 owner 挂载后按恢复触发点重传。旧 owner 不得使用已 dispose 的远端服务再失败一次，否则会把自动重试预算烧成不会自动恢复的 `failed`。
- **连接断开不是上传失败**：`MODE_REMOTE_WORKSPACE_DISCONNECTED` 直接退回 `waitingSession`，不消耗重试预算、不闪红色 `failed`。
- **runtime unavailable 作废**：中止在飞上传并清除其重试定时器与队列项；不把作废写成 `failed`。
- **手动重试**（附件上的重试图标）仍是显式用户动作：完整重置上传态与两类重试计数，优先级最高。
- 失败原因必须可读（`uploadError` 进入 title / aria-label）。生产构建 renderer 日志关闭，不能依赖日志定位此类失败。

收向：

| 场景                                    | 行为                                                                                      |
| --------------------------------------- | ----------------------------------------------------------------------------------------- |
| 超容量（非视频 >20 MiB / 视频 >30 MiB） | `previewTooLarge` → 「附件超过会话内下载上限（20 MiB，视频 30 MiB），请到工作区直接取用」 |
| ref 不匹配权威行 / 跨会话               | `previewRefNotAuthorized` → 「附件已不可用」（放权不动此边界）                            |
| 旧版本对端返回 `previewNotMedia`        | 「对端版本过低，无法下载该附件」                                                          |
| host 无 `readSessionAttachment`         | `fault.attachment.readUnsupported` 明确错误                                               |
| 下载中断                                | 可重试；`saveFile` 只在字节完整后触发，不落半截文件                                       |
| 对端断连                                | `useWorkspaceServices` rejecting proxy 现状错误                                           |

### 事件顺序

```text
发向（本地 → 远端）：
Renderer(composer)            窗口 Host(Main)                     远端机器
选文件/拖拽/粘贴 → 入队
 │ localPath + 远端 target
 ▼
runUpload ─ stage(opId) ────▶ ① 判定传输类型
                               ├ SSH/WSL/Docker: backend.upload 流式
                               │  → ~/.mode/tmp/prompt-attachments/<trace>/<nonce>/NN-file
                               ├ LAN: 流式读文件 → attachmentBegin/Chunk/Commit（代理到对端 CLI）
                               └ workspace-pick: 直接放行（已在目标机）
 ◀── onDynamicProgress ────── ② {ref, bytes, staged:true}
 ▼
sendText{attachments:[ref]} ──────────────────────────────▶ gateway 按权威行授权
                                                            ▼ mapAttachmentRefs → agent

收向（远端 → 本地）：
远端 CLI gateway ◀── attachmentRead{ref, target} ── Renderer(预览/下载)
 │ 权威行 ref 精确匹配（任意 MIME，跨 session 拒绝）
 │ readSessionAttachment（≤20/30 MiB，整读缓存切片）
 ▼
chunked bytes ──▶ media → 预览对话框；非媒体/artifact → platform.saveFile
```

## 验收场景

1. SSH 远端对话：发送 图片 + PDF + txt + 非媒体二进制（.zip/.exe，`application/octet-stream`）→ 历史中图片/PDF 可预览，全部可下载到本地（含远端机器上校验响应的 app）。
2. LAN 远端对话：同上；staged 路径 ref 的回读由远端 CLI 执行、经代理返回字节。
3. LAN 发送本地路径附件（≤20 MiB）→ host 流式上传 → `staged:true` → 远端 agent 可读；>20 MiB → 发起前拒发并给出可读文案。
4. LAN 粘贴截图（dataBase64）→ materialize 放行，正常送达（回归现状不被 guard 拦截）。
5. 远端选文件：「+」菜单在远端 target 显示入口 → DirectoryBrowser 多选文件 → 跳过 stage 直接构造 ref → 发送后 agent 可读；选目录不可提交。
6. materialize 排序判定：遗留 `sendPrompt` 路径收到「本机不存在、目标机存在」的 `localPath` → 放行不上传；两侧都不存在 → 可读错误（单测覆盖，主路径不依赖）。
7. artifact 行：文件名 chip 可下载；图片 mimeType 可预览。
8. 超限下载（非视频 >20 MiB / 视频 >30 MiB）→ `previewTooLarge` 明确提示，不静默失败。
9. 安全回归：跨 session ref、ref 不匹配权威行 → 仍拒绝（`previewRefNotAuthorized`）。
10. 本地工作区对话：下载能力同样生效（收向不区分本地/远端，顺带收益）。
11. 断连/对端能力缺失：LAN stage 中断 → composer `failed` + 可重试；对端无 put hook → 明确报错。
12. 选择窗口位置：远端会话点「从工作区选择文件」→ DirectoryBrowser 在**屏幕中央**打开（不贴屏幕下方、不被底部输入栏裁掉）。

## 测试

- 单测（gateway 授权矩阵）：userInput 行任意 MIME 授权、ref 不匹配拒绝、跨 session 拒绝、artifact 行授权与拒绝、`attachmentPreviewSource` 媒体 gate 不回归。
- 单测（发向）：LAN materialize inline 放行 / localPath 内联转换与超限；materialize 排序判定四分支（staging 根 / 本机存在 / 仅目标机存在 / 两侧都不存在）；workspace-pick 在 composer 跳过 stage；DirectoryBrowser files 模式过滤与多选。
- 单测（下载）：分段拉取拼装、错误码 → 文案映射；读响应 schema 接受任意 MIME（含 `application/octet-stream`）、结构/容量校验不回归；`attachmentPreviewSource` 的媒体 gate 不回归。
- E2E（交互改动必需）：远端工作区会话发附件（LAN 模拟）→ 历史出现 → 下载落盘；超限失败出提示。
- 验证：`pnpm typecheck`、`pnpm lint`、目标包测试按实际 `package.json` 入口执行并如实报告。

## 不做

- **任何上限调整**（已确认推迟）：上行 `attachmentMaxBytes` 20 MiB、`attachmentUploadMaxChunks` 64、`attachmentUploadMaxStagedBytes` 64 MiB、读容量 20/30 MiB 全部不动。
- 手机远控专项缺口：web 长粘贴文本附件（`createTempTextAttachment` 不支持）、conversation-share 桌面限定。
- 断点续传式下载 / range 拉取（gateway 整读缓存切片维持现状）。
- 附件中心、本地删除管理、IM bot 通道附件。
- 按连接 role 分档的上传限额（staging 仍为内存模型时无必要）。
- 协议层 origin 标记（`AttachmentRef` 加来源字段）：仅为消除遗留 ref↔prompt 转换在「两机同名不同内容路径」下的歧义，主路径不受影响，推迟到有实证场景再做。

### 附录：未来「大附件」任务已识别的完整清单

上行放开到 200 MiB 的链路瓶颈已分析，供后续任务直接引用：

1. 常量拆分：`attachmentMaxBytes` 同时卡 begin schema 与**读路径整读内存上限**，必须拆出 `attachmentUploadMaxBytes`（读侧常量不动）。
2. `attachmentUploadMaxChunks` 64 → ≥400（64 × 512 KiB = 32 MiB 硬顶先于 schema 撞上）。
3. `attachmentUploadMaxStagedBytes` 聚合预算抬升。
4. **staging 落盘化**（必要）：`attachment-upload-registry.ts` 现为纯内存 chunks 数组 + commit 再拼整 buffer（2× 峰值，200 MiB → ~400 MiB），需改临时文件 + 流式 hash/拷贝。
5. host 侧流式上传组件（LAN stage 已按流式实现，此项为前置）。
6. 收向 range 读：host `readBinaryFile` offset（gateway 注释「尚未实现」），否则 >30 MiB 无法下载。
