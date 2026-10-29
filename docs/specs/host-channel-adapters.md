# Spec：壳内频道适配器（`crates/host-channels`）

状态：2026-10-29。已适配五个频道：`memory`、`prompt-attachment-transfer`、
`client-config`、`client-scenes`、`media-preview`。
范围：**RPC 方法名与参数形状**到服务调用的翻译、错误映射。服务实现本身在各自的 crate。

## 参数形状

`ProxyChannel` 把 `channel.call(ctx, method, ...args)` 编码成
`arg = [位置参数 0, 位置参数 1, …]`。适配器统一用 `params::first_arg_object` 取第一个
对象参数，`params::required_string` 取必填字符串。对象参数在 RPC 层是 `Value::Json`
（原样 JSON 文本），`Value::to_json()` 负责解析。

## memory

| 方法 | 参数 | 返回 |
| --- | --- | --- |
| `listProjectMemories` | 无 | 工作区数组（见 `project-memory-service.md`） |
| `readProjectMemoryFile` | `{workspaceId, fileName}` | `{content, updatedAt}` |

错误映射：`MemoryError` → `ChannelError::Error`，有错误码时附 `extra["code"]`：
`PROJECT_MEMORY_PREVIEW_LIMIT_EXCEEDED` / `PROJECT_MEMORY_FILE_CHANGED`。

## prompt-attachment-transfer

| 方法 | 参数 | 行为 |
| --- | --- | --- |
| `stage` | `{operationId, localPath, sizeBytes?, …}` | 返回 `{operationId, ref: localPath, bytes, staged:false}`；`sizeBytes>0` 优先，否则 `stat(localPath)`，失败回落 `0` |
| `adopt` / `cancel` / `cleanup` | `operationId` | no-op（`undefined`） |
| `onDynamicProgress` | 订阅 `operationId` | 返回可释放订阅，**永不 emit**（本地零拷贝没有上传进度） |

未知方法/未知事件一律报错，不静默成功——否则客户端会误以为操作已生效。

## client-config / client-scenes（当前构建下的常量服务）

`packages/shared/src/officialPlatformPolicy.ts` 的 `isOfficialServiceEnabled` 目前**恒为
`false`**（官方平台服务已下线）。此时 TS 实现在读凭证与发网络请求**之前**短路：

| 频道 | 方法 | 返回 |
| --- | --- | --- |
| `client-config` | `getSnapshot` | `{"pluginStoreOrder": null}`（只此一个字段） |
| `client-scenes` | `list` | `{"code": 0, "msg": "", "data": []}` |

Rust 适配器如实复制这个分支，**不实现网络路径**。若官方服务重新启用，这两条必须与 TS
同步改造；在此之前它们只是把两个频道的所有权从 Node 侧移走，行为等价。

## media-preview

| 方法 | 参数 | 行为 |
| --- | --- | --- |
| `prepare` | `{path, expectedKind}` | 校验扩展名与 `kind` → stat 必须是普通文件 → 超过 **8 MiB** 报错 → 读全文并 base64，返回 `{kind:"inline", dataBase64, mediaType, path, size}` |

当前宿主没有注入 `authorizeLocalMediaPreviewPath`（那是远端/Electron 的 `local-url` 分支），
所以本地只走 **inline**。`refreshPlaybackUrl` / `release` 在 TS 里是可选方法，本地服务没有，
这里同样按未知方法报错。扩展名表来自 `packages/shared/src/media-preview.ts`，两边必须同步。

## 与路由的约定

- 适配器只被壳内 `ChannelServer` 调用（`inprocess.rs`），当前只接管 **base scope**；
  scoped 帧由 `FrameRouter::base_scope_only` 留给 Node Host（本地回帧不带 scope 前缀）。
- `ChannelServer` 以 `defer_init = true` 构造且从不 `ready()`：`Initialize` 只能由 Node Host 发。
- 开关 `MODE_HOST_INPROCESS_CHANNELS` 逐个列出频道名；未移植的名字被忽略并留一行日志。

## 验收

- `crates/host-channels/tests/memory_channel.rs`（4 条）：列表、读取、错误码透传、参数校验。
- `tests/attachment_channel.rs`（5 条）：sizeBytes 优先、stat 回落、三方法 no-op、
  进度订阅不 emit、未知事件报错。
- `tests/client_metadata.rs`（3 条）：两个常量返回值、未知方法报错。
- `tests/media_preview.rs`（5 条）：inline 音频 base64 与 mediaType、kind 不匹配、
  不支持扩展名、8 MiB 上限、目录/未知方法、大小写与反斜杠归一。
- `tests/pipeline.rs`（3 条）：真 RPC 帧 → 路由 → `ChannelServer` → 回帧；scoped 帧转发；
  未知方法回 `PromiseError`。
