# Bot 附件缓存的保留期与清理

状态：草案（2026-10-16）。关联：[bot-queued-message-attachments.md](./bot-queued-message-attachments.md)
（该文的非目标把本项划出范围，本行为因此单独成文）、[bot-state-ownership.md](./bot-state-ownership.md)
（队列写盘纪律；受保护集合的权威来源）。

## 背景

**缓存只增不减。** 入站附件解析后由 `cacheResolvedAttachment`（`botsService.ts:2200`）写到
`buildAttachmentCachePath`（`:2128`）生成的路径：

```
<configDir>/bot-attachments/<botId>/<messageKey>/<digest>-<filename>
```

其中 `configDir = getAppConfigDir() = <dataRoot>/v2`（`paths.ts:82`）。写完之后**全仓库没有任何一处
删除或裁剪这些文件**（搜 rm / unlink / clean / prune 在该路径上零命中）。附件物化改造之后写入点
还变多了：`prepareBotMessageContent` 与 `materializeQueuedAttachments` 都会调 `cacheResolvedAttachment`。

**手动清理也不覆盖它。** 本仓库有存储清理子系统（`storage/domain/storageCatalog.ts`），但
`v2/bot-attachments` 在 `PREFIX_RULES` 里被归入 **`config`** 类，而 `CLEANABILITY.config = "none"`
——也就是说它不仅没有自动保留期，还被**分类保护**掉了，用户在"清理存储"里也删不到。

结论：每收到一个 IM 附件就永久落一份，没有任何回收路径。这不是"存量问题可以放着"，而是
**无界磁盘增长**。

## 产品规则

### R1 保留期：非受保护文件超过 7 天删除

按 `mtime` 判定。7 天远离任何合法复用窗口（附件在当轮或队列补投时就被消费），同时给排障留了余地。

### R2 总量上限：非受保护文件合计超过 1 GiB 时，按 mtime 由旧到新删到不超

R1 只能约束稳态，挡不住短时间突发（例如一次性收到一批大图）。上限是给这种情况的兜底，
淘汰顺序固定为"最旧的先走"，保证刚写入的文件最后才可能被淘汰。

### R3 受保护集合：排队消息引用的文件永不删，且不计入 R2 配额

权威来源是 `bot-state.v4.json` 里所有对话的 `queuedMessages[].attachments[].localPath`
（`BotsStateFile → bots[botId].conversations[key].queuedMessages[].attachments[]`）。

**这条不是可选的。** 补投侧 `resolveAttachmentBytes` 的第一跳就是 `readFile(localPath)`，
而队列补投的 `handleMessage` 包在一个 try/catch 里、失败只 warn 后 `return`——也就是说
**附件文件缺失会让整条排队消息（含正文）一起丢掉**，不是只丢附件。受保护集合因此必须
"宁多勿少"：拿不准的路径一律当受保护。

### R4 触发与失败处理

- 首个入站前执行一次，之后**每进程每小时至多一次**（进程内节流；节流状态可丢，重启后多跑一次无害）。
- 任何一步失败（读状态、枚举、删除）都只 `warn`，**不阻断入站**。
- 删除用 `force` 语义：文件已不在视为成功，因此两个窗口并发清理时重复删同一文件不会报错。

### R5 消费侧容错：localPath 缺失降级为"解析失败"

`resolveAttachmentBytes` 读 `localPath` 遇 `ENOENT` 时返回 null，走**既有**的"附件未能下载"分支
（`prepareBotMessageContent` 已实现：把附件行标注为未能下载并继续）。

理由：R3 是防"误删"，但清理是异步的、跨进程的（多窗口共享同一个数据目录），
R3 不可能给出绝对保证。这一条把"误删 → 整条消息丢失"降级成"误删 → 丢一个附件"，
代价是一处 catch，不新增分支形态。

## 状态所有者与时序

| 数据 | 所有者 | 持久化 | 可丢性 |
| --- | --- | --- | --- |
| 缓存文件 | 文件系统；唯一写者 = `cacheResolvedAttachment` | 是 | 可丢（除 R3 受保护的那些） |
| 受保护集合 | `bot-state.v4.json` 的 `queuedMessages`（**不新增状态**） | 是 | 不可丢（队列本体） |
| 清理节流时间戳 | 进程内变量 | 否 | 可丢 |

```
入站 → processProviderCallback（唯一入站入口）
  ├─ ensureGroupHistoryLoaded()          （已有）
  └─ pruneAttachmentCacheIfDue()         （新增，进程内节流 1h）
        ├─ repo.readState()  → 收集 queued 附件的 localPath = 受保护集合
        ├─ 递归枚举 <configDir>/bot-attachments
        ├─ planAttachmentCacheEviction(entries, protected, now)  ← 纯函数
        └─ 删除（rm force）+ 回收空目录；失败只 warn
```

## 接口

- `packages/services/src/bots/attachmentCacheRetention.ts`：策略常量 +
  `planAttachmentCacheEviction(params)`（纯计算，只回答"删哪些"，删除动作留调用方，便于单测）。
- 清理接线在 `botsService.ts` 内部，不新增 `IBotsService` 方法（没有 UI 面，也不需要用户触发）。

## 验收场景

1. **过期即删**：造一个 mtime 为 8 天前的非受保护文件 → 跑一次 → 文件被删，随之变空的目录被回收。
2. **受保护不删**：队列里引用着一个 8 天前的文件 → 该文件保留，同批次其余过期文件照删。
3. **上限淘汰**：非受保护文件合计超 1 GiB → 按 mtime 由旧到新删至不超；受保护文件仍纹丝不动。
4. **失败不阻断**：删除抛错（文件被占用等）→ 不抛到入站链路，只 warn。
5. **消费侧降级**：队列引用的文件被人为删掉 → 该条排队消息**仍投递正文**，附件降级为
   「未能下载」，不再整条丢失。
6. **节流**：同一进程内连续两次入站 → 只实际清理一次。

## 非目标

- **不改存储清理的目录分类**（`v2/bot-attachments` 维持 `config` / `none`）。把它改成可清理
  属于独立决策：要么新增类别，要么改归类并连带调整 UI 的分类文案与安全级别。
- **不做用户可配的保留期 / 上限**。常量即可；要开放成设置项需另立 spec（含 UI 与文案）。
- **不做跨进程互斥**。见 R4：`rm force` 幂等，最坏情况是两个窗口各尝试一次。
- **不改队列上限、去重规则与物化时机**（归 `bot-queued-message-attachments.md`）。
