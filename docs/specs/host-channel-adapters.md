# Spec：壳内频道适配器（`crates/host-channels`）

状态：2026-10-29。已适配九个频道：`memory`、`prompt-attachment-transfer`、
`client-config`、`client-scenes`、`media-preview`、`conversation-mirror`、`setting`、
`credential`、`system`。
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

## conversation-mirror

| 方法 | 参数 | 返回 |
| --- | --- | --- |
| `getMirrorIdentity` | 无 | `{deviceId,label}` |
| `publishSnapshot` | `{snapshot}` | `{stored,reason?,recordCount}`（origin 必须是**别人**） |
| `writeLocalSnapshot` | `{snapshot}` | 同上（origin 必须是**自己**） |
| `readWorkspaceSnapshots` | `{workspacePath}` | `{snapshots:[…]}` |

存储、校验与有界化语义见 `conversation-mirror.md`；适配器只做参数解析与 `spawn_blocking`。

## setting

| 方法 | 参数 | 返回 |
| --- | --- | --- |
| `get` | 无 | 完整 `AppSettings`（默认值合并后） |
| `update` | `patch` 对象（第二个参数 `expectedAccountSettings` **当前被忽略**） | `undefined` |
| `updateDataBaseDir` | 位置参数 `newDir`（空/缺省 → 用户 home） | `undefined` |
| `ensureDefaultProject` | 位置参数 `homedir`（缺省 → 用户 home） | `{path, created}` |

数据目录迁移的复制规则、禁写校验与写入顺序见 `settings-store.md`。

**与 TS 的已知差异（开启开关前需补齐或明确降级）**：

- 无写队列：并发 `update` 不串行化，后写补丁可能覆盖先写字段。
- 无 `expectedAccountSettings` 账号 CAS；第二个参数被丢弃。
- 补丁不做 schema 类型校验，非法值在合并时逐字段回落（TS 会整条抛错）。
- 数据目录指针持久化的是**解析后的目标路径**（清空时回落 home），TS 持久化调用方原值。
- `updateDataBaseDir` 的禁写错误不附 `extra["code"]`；TS 会带
  `DATA_BASE_DIR_FORBIDDEN_WINDOWS_INSTALL_DIR`。

## credential

| 方法 | 参数 | 返回 |
| --- | --- | --- |
| `load` | 位置参数 `key` | JSON `"value"` 或 `null` |
| `save` | 位置参数 `key`, `value` | `undefined` |
| `delete` | 位置参数 `key` | `undefined` |
| `list` | 位置参数 `prefix` | `string[]`（键名升序，不解密） |

四个方法都是同步文件 IO（锁可能等待），统一丢进 `spawn_blocking`。文件层、密文格式与
密钥派生见 `credential-store.md`。

**与 TS 的已知差异**：

- 解密失败同时带 `code = MODE_CREDENTIAL_DECRYPT_FAILED` 与「凭据解密失败：」消息前缀，
  渲染层两种识别方式都能命中。
- 键校验失败的文案是固定英文，不是 zod 文案（非契约）。
- Host 私有的 `onDidMutate`（`onProviderProvisioningSourceChanged`）当前**没有调用方**
  （`packages/services/src/node.ts` 的选项未接线），壳内不转发；接回该通知时要同时补桥接。
- 文件按字典序写回（`serde_json` 默认 Map），语义等价。

## system

| 方法 | 参数 | 返回 |
| --- | --- | --- |
| `info` | 无 | `{homedir, platform}` |
| `listIntegratedTerminalShells` | 无 | `IntegratedTerminalShellOption[]` |
| `probeIntranet` | 位置参数 `request` 对象 | `IntranetProbeResult` |

`probeIntranet` 的归一化、重试与聚合见 `system-service.md`；适配器只做对象解析与
`spawn` 语义（探测本身在 tokio 里 await）。

**与 TS 的已知差异**：连接失败的错误文本、HTTP 客户端（reqwest/rustls）、重定向上限
（10 vs 20）、homedir 的最后一级兜底；都只影响诊断字段，不是接口契约。

## 与路由的约定

- 适配器只被壳内 `ChannelServer` 调用（`inprocess.rs`）；base scope 与
  `scope-ready` 的 remote scope 分别持有独立服务面实例，scoped 回帧由
  `encode_scoped_frame` 加 `0xFF + scopeId` 前缀。
- `ChannelServer` 以 `defer_init = true` 构造且从不 `ready()`：`Initialize` 只能由 Node Host 发。
- 开关 `MODE_HOST_INPROCESS_CHANNELS` 逐个列出频道名；未移植的名字被忽略并留一行日志。

## 验收

- `crates/host-channels/tests/memory_channel.rs`（4 条）：列表、读取、错误码透传、参数校验。
- `tests/attachment_channel.rs`（5 条）：sizeBytes 优先、stat 回落、三方法 no-op、
  进度订阅不 emit、未知事件报错。
- `tests/client_metadata.rs`（3 条）：两个常量返回值、未知方法报错。
- `tests/media_preview.rs`（5 条）：inline 音频 base64 与 mediaType、kind 不匹配、
  不支持扩展名、8 MiB 上限、目录/未知方法、大小写与反斜杠归一。
- `tests/conversation_mirror_channel.rs`（2 条）：身份与两条 origin 规则、读取回环。
- `tests/setting_channel.rs`（3 条）：默认值读取 + 补丁落盘、默认项目只建一次、
  数据目录迁移写指针。
- `tests/credential_channel.rs`（5 条）：save/load（含 null）/delete/list 往返、缺参报错、
  解密错误的 code 与消息前缀、坏文件不被覆盖、未知方法报错。
- `tests/system_channel.rs`（6 条）：info 字段、非 Windows 空 shell 列表、
  探测结果的 camelCase 形状、空目标、畸形请求、未知方法。
- `tests/channel_names.rs`（漂移守卫）：频道名与 TS `ServiceChannels` 一致。
- `tests/pipeline.rs`（3 条）：真 RPC 帧 → 路由 → `ChannelServer` → 回帧；`base_scope_only`
  路由把 scoped 帧留给 Node Host；未知方法回 `PromiseError`。
