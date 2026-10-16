# 远程工作区对话记录镜像（Conversation Mirror）

状态：已实现。挂载点已随 v0.0.43 发版；2026-10-15 补修「冷会话读不出消息」导致镜像导入恒为 0 的缺口（见下节）；
同日第二轮修复「`session/messages` 返回值不是 wire 形状导致读取必失败 / ②方向来源 scope 取错 /
只在挂载时跑一轮 / 导入时间线被压平」四处（见「2026-10-15 第二轮修复」）；
同日第三轮按产品口径把单轮窗口提到**至少 10 条**，并把导入会话的**活跃 record 时间线回填**，
让镜像在侧栏按来源的真实时间轴排列（见「2026-10-15 第三轮调整」）；
同日第四轮把**同步本身的开销与轮次并发**收口（见「2026-10-15 第四轮修复」）——
前几轮把「对不对」修完了，这一轮修的是「跑不完」；
2026-10-16 第五轮按「读一次要多久」与「失败怎么归类」继续收口（见「2026-10-16 第五轮修复」）：
`limit` 下推到会话消息查询、来源列表按**可用条数**补页、导入失败区分「对端真的不认这个导入源」与「暂时性错误」。

**发版事实（必读）**：第四轮修复此前只存在于工作区、从未提交，因此 **v0.0.52 及更早的已安装版本都不含第四、第五轮**；
第四轮之前只有第一至第三轮随 v0.0.43 / v0.0.45 / v0.0.46 发过。这两轮必须重新发版才在客户端生效，
且第四轮的轮次串行化与读取退避需要**两端都升级**才能观察到实际效果。

## 2026-10-16 第五轮修复（读取量级 / 来源列表补页 / 导入失败分类）

第四轮修掉了「跑不完」（轮次自激、空转轮满负荷、失败无出口）。第五轮修的是第四轮之后仍然残留的
「慢」与「有时候不同步」——它们都不是并发问题，而是**单次操作的成本量级**与**失败的分类口径**。

**成因 A（读的量级是整条会话，不是窗口）**：镜像单次读传 `limit: 200`，但 CLI 的 `readMessages` 一直是
「把 store 里的消息全量取出 → 找锚点 / 切片 → 最后才 `slice(-limit)`」；而 `sessionStore.messages({ sessionID })`
的入参只有 `sessionID`（`repositories/messages.ts` 两条**不带 LIMIT** 的 `select *`，再逐行
`decodeMessageRow` / `decodePartRow`）。于是单次读取的耗时与**会话历史长度**成正比，而不是与窗口成正比——
第四轮把并发排队去掉了，每次读取的量级没变，长会话上依旧慢。

修法：`sessionStore.messages()` 增加可选 `limit`，语义 = 取**最新** N 条（逆序查询后翻回正序，parts 用同一逆序
子查询过滤），与「全量读 + `slice(-N)`」逐项相等。`readMessages` **只在下推语义成立时**才交出去：

- 无 `afterMessageId`：有锚点时锚点可能落在尾部窗口之外，必须先全量读再切片；
- 会话**不带 conversation rewind 账本**（`applyRewindBranch` 会按 `created/target` 裁剪历史，输入被截断结果会不同）；
- `limit` 非正 / NaN / 非整数一律退化成全量读（坏参数不能把会话读成空）。

活跃路径保持全量读：它还要套 runtime 的 rewind 事件投影，且活跃会话通常只有一条。

**成因 B（来源列表一页可能装不下 windowSize 条可用来源 → 更旧的来源永远同步不过来）**：来源列表按
`updatedAt` 取最新 N 条，而镜像副本会长期留在来源 scope 里，且它们的 `updatedAt` 是**来源的真实时间**
（回填后），排位靠前。原来固定取一页（`maxRecords + windowSize` = 30–40 条），于是对端本地 scope 的头部被
副本占满时，这一页里真实来源可能不足 10 条——**更旧的来源既不会被导入也不会报错**，静默地永远漏掉。

修法：`readScopeTasksForWindow` 按**可用条数**（而不是总条数）判停，不足 `windowSize` 就逐级翻倍补页
（30 → 60 → 120 …），上限 `MIRROR_SOURCE_DEEP_READ_LIMIT = 200`；这一页本来就不满（列表到底了）或到上限即停。
可用性谓词与两个方向的选择器同源：① `isMirrorEligibleTask && !isMirrorSessionId`，
② `!isMirrorSessionId`（保留 identity 行）。

**成因 C（导入失败的分类太粗 → 一次抖动把整轮判死并报错成「版本过低」）**：`importScopeRecords` 原来是
「首个 `createSession` 失败即返回 `unsupportedError`」，调用方据此把整轮判成 `unsupported`。但
`createSession` 的失败也可能只是 RPC 超时（默认 3 分钟）、连接中断、对端繁忙、磁盘错误——这些是可重试的，
却被当成「对端版本过低，无法同步对话记录」，并且中止该方向。

修法：`isUnsupportedImportSourceError` 只认「载荷被对端 schema 拒收」——错误码 `-32602`（旧 CLI 的 strict
schema 把 `importedHistory` 判为 invalid params，校验路径会出现在 message 里）或 `-32601`（更旧的 CLI 连
`session/create` 都没有），或错误文案点名 `importedHistory` / `remoteMirror`。其余失败逐条跳过并计入
`importFailures`；本轮 0 导入且存在这类失败时报 degraded（与读取失败、退避跳过同样不能被洗成 ready）。

**仍未实机验证**：成因 A 的收益取决于来源会话长度，需要真实长会话才能观察到量级差异（本机单测钉住的是
「语义等价」与「何时下推」）；成因 B / C 需要构造「来源 scope 混着大量镜像副本」「对端暂时不可用」的真实场景。
两端升级依旧是前提（读取是被读的那台机器在服务，导入落在承载工作区的那台机器上）。

## 2026-10-15 第四轮修复（同步自激：读不完的轮次）

**症状**：连上远端工作区后侧栏长期停在「正在同步远端对话记录...」，几小时不结束；当日日志涨到 231 MB
（前几日 5–8 MB），单行最大 2.9 MB。实测（本机 host 日志）：`mode-session.readSessionMessages`
当天成功 1032 次 / 失败 421 次，且**单次读取耗时随时间恶化**——20 点均值 3 s，21 点 707 次调用均值 139 s，
全天中位数 92 s、P90 289 s、最慢 359 s。

**三个叠加成因**（互相放大，单独修任何一个都还会卡）：

- **成因 A（空转轮和真干活轮一样贵）**：`importScopeRecords` 先读消息、**之后**才判水位，
  于是「目标端已是最新」的轮次照样把 `windowSize`（10–20）条会话的全部消息（每条最多 200 条）读一遍。
  读之前就能判定的事，被放到了读之后。
- **成因 B（轮次可以重叠，且会自我放大）**：`useWorkspaceConversationMirror` 收到来源 scope 事件就开新一轮，
  **不检查上一轮是否还在跑**；间隔下限按「距上轮**开始**」计，而一轮要几分钟，于是每个事件都叠一轮，
  并发读堆在 host 上互相排队，单次读取越等越久——读取更慢又让轮次更长，正反馈。
- **成因 C（失败没有出口）**：`file` part 的 `url` 在按路径引用附件时就是空字符串
  （生产者：`apps/mode-cli/packages/core/src/runtime/helpers/attachments.ts` 的 `url: attachment.content ?? ""`，
  连 `media-observability.ts` 都把 `"empty"` 单列一类），而 wire 合同 `mode-protocol-legacy-types.ts` 的
  `file` 变体把 `url` 声明成 `nonEmptyString`。只要有这么一条 part，**整条 `session/messages` 响应校验失败**，
  该会话永远读不出来；镜像又没有负缓存，于是每一轮都重试同一条会话并打印包含全部校验 issue 的错误
  （这就是 2.9 MB 单行日志的来源）。

**修法**（对应上面的 A/B/C，加一条日志治理）：

1. **水位前置**：新增共享纯函数 `shouldReadMirrorSource({task, targetUpdatedAt})`，在**读消息之前**判定；
   读之后原有的 `shouldImportMirrorRecord` 保留为权威兜底。两者同源——`buildConversationMirrorRecord`
   的 `updatedAt` 直接取 `task.updatedAt`，所以前置判定与后置判定是同一条判据，前置等价、只是省掉一次读。
2. **单轮串行化**：同一 workspace 同一时刻只跑一轮；轮内到达的事件只置「待重跑」，等当前轮结束再补跑一次
   （不是丢掉，也不是叠一轮）。串行化后「间隔下限」不再是防自激的主力，退化为抖动抑制。
3. **读失败退避**：新增 `createConversationMirrorReadBackoff()`（Renderer 侧，按来源 taskId 记退避），
   失败一次退避 60 s 起、指数翻倍、上限 10 min，成功即清零。退避期内的来源不再重读，但**仍计入状态**：
   本轮 0 导入且存在「读取失败 + 退避跳过」时照旧报 degraded（不能让持续失败被退避洗成 ready）。
   手动「重新同步」会先清退避，保证重试是真重试。
4. **wire 容忍空 url**：`file` 变体的 `url` 从 `nonEmptyString` 放宽为 `z.string()`，与生产者契约
   （`session-store.port.ts` 的 `url: string`）一致。消费端本来就按真值判断（`ai-elements/attachments.tsx`
   的 `data.url`、`ChatMediaAttachmentPreviewDialog` 的 `attachment?.url`），空串等价「不可渲染」，
   不会把 `<img src="">` 发出去。
5. **日志有界**：`packages/rpc/src/logging-middleware.ts` 在打 FAIL 前把错误裁剪成
   `{name, message(截断), stack(截断)}`；镜像的 warn 也改为带上错误文案的字符串（原来传原始 error 对象，
   序列化成 `{}`，既丢了诊断信息又可能把整段校验明细塞进日志）。

**仍未实机验证**：需要两端都升级才能观察轮次串行化与退避的实际效果（本轮的验证是单测 + 本机日志口径）。

## 2026-10-15 第三轮调整（一轮 10 条 + 按真实时间轴排序）

两条产品口径：

**① 一轮至少同步 10 条**。窗口基准原本直接取侧栏可见条数，而它默认只有 5
（`WORKSPACE_TASK_PAGE_SIZE`），于是刚连上的远端工作区只搬过来 5 条对话，剩下 5 条要等用户点一次
「展开更多」。现在窗口口径收敛到 shared 的唯一函数 `resolveConversationMirrorWindowSize`：
**下限 10 条**（`CONVERSATION_MIRROR_LIMITS.minWindowSize`）、仍随可见条数增长、上限 20 条
（`maxRecords` 不变，远端 RPC 参数有界）。「展开更多」依旧把更旧的记录带进来（验收场景 9 不变）。

**② 同步过来的对话按来源的真实时间轴排序**。会话列表的 `updatedAt` 取自 sessions-index 的
`lastActivityAt`，而它的唯一事实源是活跃 record 的 `record.updatedAt`（v4-bridge `getSessionIndexMeta`），
再经 `modeTaskIndexSyncer` 落到 tasks-index 的 `updatedAt`、最后由
`compareModeTaskListItems(..., "updated")` 决定侧栏顺序。问题出在
`materializeSessionRecord` 把 `createdAt/updatedAt` 写死成 `Date.now()`：导入会话不回填，
`lastActivityAt` 就是**导入那一刻**，同一轮导入的 10 条于是按**导入顺序**（`selectMirrorSourceTasks`
是 updatedAt 降序，即从新到旧）排列，落到侧栏正好**新旧颠倒**；重启后会话变冷、改读
`session.time.updated`（真实时间）又会翻回来，同一批会话在两端 / 两次启动间顺序不一致。
修法：`persistImportedSessionHistory` 落盘后调 `applyImportedHistoryTimelineToRecord`，把
`createdAt/updatedAt` 回填成 `resolveImportedHistoryTimestamps` 的结果——与 `resumeSession`
对冷会话的回填同一裁决（那边早已这么做，注释见 `server-operations.ts`）。此后用户在镜像会话里
续聊，正常活动事件照旧把 `record.updatedAt` 推到现在，语义不变。

**仍未实机验证**：需要两端都升级才能观察到 10 条窗口与镜像的真实时间轴排序。

## 2026-10-15 第二轮修复（同步失败 / 两端不一致 / 单边可见 / 无分页）

线上反馈四条互相关联的症状：①本机同步远端对话记录**失败**；②本机在远端工作区发起的对话，对端看不到；
③对端发起的对话，本机看不到（或只在连接那一刻看得到）；④同步完两端不一致。日志（`~/.mode/v2/logs`）
与源码定位出四个独立成因：

**成因 0（`session/messages` 返回值不是 wire 形状 → 症状①，本轮的主因）**：
`readMessages` 把 session store 的内部行直接当响应返回——store 行是 `info.id` / `part.messageID`，
而 wire 合同 `modeSessionMessagesResultSchema` 要的是 `info.messageId`。调用方
（`modeAgentService.readSessionMessages` 的 `resolveResponse`）逐条校验，于是**只要会话里有消息就必定
失败**（只有空会话碰巧通过）：

```text
mode-session.readSessionMessages FAIL
{"name":"ZodError","message":"[... {\"code\":\"invalid_type\",
  \"path\":[\"messages\",0,\"info\",\"messageId\"],
  \"message\":\"Invalid input: expected string, received undefined\"} ...]"}
```

后果正是「本机同步远端对话记录失败」：镜像逐条读失败 → 本轮 0 导入 → 状态降级。
这同时说明 2026-10-15 的冷读修复只解决了「读不到冷会话」，没解决「读到了也发不出去」——两处缺口叠加，
单独修任何一处都仍然是 0 导入。
修法：`readMessages` 在切片之后统一过 `mapMessageWithParts`（快照路径一直在用这同一个 mapper，
`session-mapper.ts`），活跃路径与冷读路径同时修正。
这条 RPC 全仓只有镜像在调用，因此影响的正是本功能。

**成因 A（②方向来源 scope 取错 → 症状②）**：②方向原本只读「本机在同路径下的本地 scope」
（`tab.workspacePath`）。双机路径不同（远端 `D:\gas-hub-xp` 对本地 `D:\ZCodium`）时该 scope 恒为空，
于是 **本机连上远端工作区后在其中发起的对话永远到不了对端**——那些会话落在「以对端身份打开的那个
scope」（`{workspacePath, workspaceIdentity: 本机 attach 身份}`）里，原本被 `isMirrorEligibleTask`
（要求无 identity）直接过滤掉了。
修法：②方向的来源改为**本机在该工作区的全部记录**= 本机本地 scope（无 identity）∪ 本机视图 scope
（带 identity），两者合并去重；`mirror-` 前缀仍跳过，防回声与第三方扩散不变。
①方向不变：来源仍必须是对端本地 scope（带 identity 的行是对端的内容，转发即泄漏，见不变量 2）。

**成因 B（只在挂载时跑一轮 → 症状②③）**：`useWorkspaceConversationMirror` 只在
`workspaceKey + 连接代次 + 窗口`变化时跑一轮，而 `WorkspaceConversationMirrorStatus` 在 `ready` 下
渲染 null（连手动刷新入口都没有）。结果是连接之后任一端新开 / 续聊的对话都同步不过去。
修法：订阅两个来源 scope 的 `workspace_task_list_changed`（对端本地 scope 是①来源、本机视图 scope
是②来源），防抖 1.2s 后补跑一轮；相邻两轮另有 20s 间隔下限——镜像自身的写入也会发事件，而
「水位是否前进」取决于对端 CLI（见成因 C），间隔下限保证对端为旧版本时最坏情况是低频空转而不是持续重导。

**成因 C（导入时间线被压平 → 症状①④）**：CLI 的 `persistImportedSessionHistory` 里
`updatedAt` 只对 `claudeCode` 透传，其余来源（含 `remoteMirror`）一律退化成 `createdAt`。
而目标 scope 的 task meta `updatedAt` 正是「已同步到哪一版」的水位，于是：镜像会话在两端的
创建 / 更新时间与排序不一致（两端不一致）；且水位永远停在来源会话的创建时刻，**每轮同步都判定
「来源更新」而重导一遍**——配合成因 B 的自动重跑会变成持续重导的活锁。
修法：抽出 `resolveImportedHistoryTimestamps`，`claudeCode` 与 `remoteMirror` 同等对待
（透传 `updatedAt`，缺失时用末条消息时间兜底）。这是成因 B 能安全上线的前提。

**成因 D（无分页 → 症状「同步最近 5 条 / 展开更多再加 5 条」）**：单轮固定按
`CONVERSATION_MIRROR_LIMITS.maxRecords = 20` 灌满。现改为按调用方传入的 **`windowSize`** 处理，
窗口取侧栏该工作区的可见条数（`WORKSPACE_TASK_PAGE_SIZE = 5` 起，`expand more` 每次 +5）：
窗口变大时 `useWorkspaceConversationMirror` 的 runKey 变化，自动按新窗口重跑一轮。

**仍未实机验证**：本改动需要两端都升级才能观察到 `mirror-` 会话双向落地；单测覆盖的是编排与投影。

## 实现状态（2026-10-15 核查与修复）

链路各段均已落地：协议 `packages/shared/src/mode-protocol/index.ts`（`source: z.literal("remoteMirror")`）、
共享逻辑 `packages/shared/src/conversation-mirror.ts`、UI 编排 `packages/ui/src/lib/conversationMirrorSync.ts`、
CLI 通用持久化 `apps/mode-cli/packages/bootstrap/src/mode-protocol/server-operations.ts` 的
`persistImportedSessionHistory`（`remoteMirror` 走通用分支，无专属分支）。

**曾经不生效的原因**：唯一触发组件 `packages/ui/src/WorkspaceConversationMirrorStatus.tsx` 的挂载点
被 `8242a576`（在 v0.0.36 基线上重建主线）摘掉——该提交重写 `packages/ui/src` 时删掉了
`WorkspaceSidebarItem.tsx` 里对它的 import 与渲染，组件沦为孤儿，于是
`useWorkspaceConversationMirror` → `runConversationMirrorSync` → `session/create` + `importedHistory`
整条链永不触发（功能代码在、调用点没了；typecheck / lint 不会报，因为孤儿组件是合法代码）。
2026-10-15 已在 `WorkspaceSidebarItem.tsx` 的 `CollapsibleContent` 内、`TaskList` 之后补回该挂载，
与重建前的旧树（`backup/main-before-rebuild`）一致。

**该挂载是整条链的唯一触发点**，且位于 `CollapsibleContent` 内——Radix 折叠时会卸载子树，所以自动同步
实际由「工作区行展开」门控。这条门控方向上与不变量 7（只处理用户正在使用的工作区）一致，而「激活工作区
必然展开」（`tabStore` 的 `activateTab → ensureWorkspaceExpanded`）保证了连上远端就会同步一轮，因此挂载位置
保持不动；代价是改动该处结构时必须重新确认这条依赖——孤儿组件是合法代码，typecheck / lint 都不会报。

**版本事实**：v0.0.39 / v0.0.40 含该挂载（重建前主线）；v0.0.41、v0.0.42（= `origin/main`）不含。
所以本修复必须重新发版才在客户端生效，只改工作区对已安装的 app 没有影响。

**测试**：UI 编排单测已补（`packages/ui/test/conversationMirrorSync.test.ts`，11 例，见「测试」一节）。
CLI 侧有 `resolveImportedHistoryTimestamps` 的水位单测；`session/create` 端到端落盘用例仍缺。

## 2026-10-15 冷读修复（镜像此前恒导入 0）

**症状**：链路全部就位、挂载点也在，但一轮同步导入 0 条，且 UI 与日志都看不出异常。实测（v0.0.43，双机互连）：本机 host 当天处理 239 次 `mode-session.readSessionMessages`——该 RPC 全仓只有镜像在调用——**0 次成功**，全部 `Session is not active`；CLI 侧对应 140 条 `mode_protocol.session.require_missing`（`activeSessionCount: 0`、`hasSessionStore: true`），`session/create` 为 0。

**根因**：`readMessages` 先调 `requireSession`，而它只查 `context.sessions`（活跃 runtime），冷会话直接抛错。数据其实一直都在 store 上——同一文件里的本地 helper `readSessionMessages` 本来就是 `sessionStore.messages(...)`。于是「对端会话没被 resume 过」时，①方向必然逐条读失败、`continue`，本轮 `importedCount: 0` 且仍返回 `ready`。

**修法**：`readMessages` 改为冷读容忍——命中活跃 runtime 走原路径，未命中则只读 session store（`sessionStore.getSession` + `sessionStore.messages`）再套既有投影。**不物化 runtime**，因此不变量 7 原样成立。

**两处必须如实记录的语义边界**：

- 冷读**不等价于**活跃读：事件存储是内存态（`create-app` 的 `createInMemorySessionEventStore`），冷会话没有 rewind 事件可读，所以冷读只保留「落盘 revert」那一层投影。镜像只需要 role + 文本 + 时间戳，可接受；但不要把两者当同一份结果。
- 冷读刻意用 `sessionStore.getSession` 而非 `getPersistedSession`：后者会顺带修复旧版远端工作区路径，属于写操作，被动读取不该触发对端数据变更。

**仍未补的一处（不阻断主链）**：`WorkspaceConversationMirrorSection.tsx`、`ConversationMirrorRecordDialog.tsx`
只存在于 `.plan/recovered/`（会话库恢复产物），源码树没有；本 spec 的设计不需要它们
（镜像会话本身进任务列表，无需记录列表视图）。

## 产品规则

### 范围

- 覆盖桌面窗口发起的**全部远端连接类型**（SSH / WSL / Docker / LAN）。
- 「对话记录」= 对端机器在**同一工作区路径**下、以**本机本地 scope**（`workspaceIdentity` 为空）产生的会话。
- **镜像产物是真会话，不是只读投影**：镜像记录以 `session/create` + `importedHistory` 导入成目标 scope 下的真实会话，
  与本地对话完全同质——可继续对话、可置顶 / 归档 / 改名 / 删除、进任务搜索与 `@` 提及。
- 「双向」= 一轮同步同时做两件事：① 对端记录 → **本机当前视图 scope**；② 本机记录 → **对端本地 scope**。
- 「每次连接时自动同步」= 远端 workspace attach 就绪（`rpcReady`）时自动跑一轮；之后**任一来来源 scope
  发生变更（新开 / 续聊任务）时防抖补跑一轮**，因此连接之后新产生的对话也会传过去。
  **补跑不叠轮**：同一 workspace 同时只跑一轮，轮内到达的事件合并成「结束后补跑一次」；
  **空转轮先判后读**：目标 scope 的 task meta 水位不前进就不读消息（见「第四轮修复」成因 A/B）。
- **窗口有界且随可见列表增长**：一轮只处理每个来源最近 `windowSize` 条会话。`windowSize` 以侧栏该工作区
  当前可见的会话条数（默认 5，`展开更多` 每次 +5）为基准，经 `resolveConversationMirrorWindowSize`
  归一为**至少 10 条、至多 20 条**：刚连上就同步最近 10 条，窗口变大时按新窗口重跑，
  不会一次性把远端历史灌满。
- **同步过来的对话按来源的真实时间轴排序**：导入会话的 `createdAt/updatedAt` 取来源会话的真实时间线
  （不是导入时刻），因此侧栏顺序与来源机器一致，也与重启后的冷启动顺序一致（见「第三轮调整」②）。

### 归属与执行位置（本 spec 的核心裁决）

| 方向                | 目标 scope（会话落在哪）                           | 之后续聊时 agent 跑在哪    |
| ------------------- | -------------------------------------------------- | -------------------------- |
| 对端记录 → 本机视图 | **对端机器**上、当前 attach 使用的 workspace scope | 对端机器（工作区所在机器） |
| 本机记录 → 对端视图 | **对端机器**的本地 scope（不带 identity）          | 对端机器（工作区所在机器） |

- 镜像会话一律落在**承载该工作区的机器**上，因为续聊要在有代码的那台机器执行；本机不为远端工作区另建本地副本。
- 由此继承两条例证：① 断开连接后镜像会话与远端工作区普通会话一样不可见（不是本地离线副本）；
  ② 对端机器打开该工作区时，会看到本机记录导入进来的会话（这就是「互相看到」）。
- **一个来源记录只会被导入到一个 scope**（按方向固定），因此确定性 id 不需要带 scope，重连换端口也不会产生重复。
- ②方向的**来源**是「本机在该工作区的记录」，由两个 scope 合并：本机本地 scope（无 identity，同路径本地工程）
  与**本机视图 scope**（`{workspacePath, workspaceIdentity: 本机 attach 身份}`）。后者是本机连上远端工作区后
  在其中发起的对话——只取本地 scope 会让②方向在双机路径不同时恒为空，本机发起的远端对话就到不了对端
  （见「2026-10-15 第二轮修复」成因 A）。
  明确**不采用** `localWorkspacePath`：它是「本地 → 远端」的配置同步锚点（技能 / MCP / 插件），
  两侧通常不是同一个工程，拿它当会话来源会把 A 工程的对话灌进 B 工程的工作区。

### 状态所有者

| 状态                                     | 所有者                                                                                 |
| ---------------------------------------- | -------------------------------------------------------------------------------------- |
| 镜像会话内容                             | 承载工作区那台机器的 CLI session store（普通会话，无特殊所有权）                       |
| 任务列表状态（置顶/归档/删除/未读/标题） | 该机器的 tasks-index（镜像导入**不写** task meta，产品态由用户操作独占）               |
| 导入编排与同步状态                       | Renderer hook `useWorkspaceConversationMirror` + `conversationMirrorStore`（纯 UI 态） |
| 「是否已导入」判据                       | 目标 scope 自己的 task meta `updatedAt`（无需额外台账）                                |

### 不变量

1. **镜像只导入、不复制产品态**：导入只调用 `session/create`（含 `importedHistory`），不改 task meta。
   因此重同步不会覆盖用户已设置的置顶 / 归档 / 删除 tombstone / 标题覆盖（tasks-index 只补缺失行，负向 membership 不复活）。
2. **按下方向过滤带 `workspaceIdentity` 的行**：①方向的来源只能是对端机器的本地 scope，
   带 identity 的行属于「以对端身份打开的工作区」，转发它等于转发对端内容；
   ②方向的来源反过来**必须包含本机视图 scope 的 identity 行**（那正是本机在远端工作区发起的对话）。
   两个方向的过滤条件不同，不能共用一个谓词（见「2026-10-15 第二轮修复」成因 A）。
3. **镜像副本不再被转发**：镜像会话 id 统一为 `mirror-<originSessionId>`（直接拼来源 id，不做哈希——
   Renderer 侧不能依赖 `node:crypto`，且日志与库里能直接看出镜像来源），读取对端本地 scope 时跳过该前缀。
   这同时挡住「本机副本被对端搬回来」的回声和第三台机器副本的扩散。
4. **幂等**：确定性 id + CLI 的 `removePreviousImportedSessionHistory`（按 `metadata.migrationSource` 清理旧导入消息）
   保证重复导入只覆盖导入消息、**保留用户后续在镜像会话里真实聊出的消息**。
5. **有界**：单轮每个方向处理 `windowSize` 条会话，`windowSize` = 侧栏可见条数取下限 10、上限 20
   （见「2026-10-15 第三轮调整」①，成因 D 定的是窗口可随可见列表增长这件事）；
   每条最多 200 条消息、单条文本 ≤ 4000 字符、单条记录 ≤ 256 KiB（超限丢最旧消息并置 `truncated`），
   为远端 RPC 物理帧（1 MiB）留余量。
   单次读取只取最新 200 条：`limit` **下推到存储查询**，因此单次读取成本与窗口成正比、与会话历史长度无关
   （见「2026-10-16 第五轮修复」成因 A）。
   来源列表按**可用条数**补页，深度上限 `MIRROR_SOURCE_DEEP_READ_LIMIT = 200`（见成因 B）。
6. **失败不阻断**：导入失败只记 warn + 视图状态，不阻断 workspace 连接、任务列表与会话主链路；
   对端 CLI 不认识 `remoteMirror` 时判为「对端版本过低」，本轮不再重试，UI 给出可读提示。
   **这个判定只认「载荷被对端 schema 拒收」**（`-32602` / `-32601`，或错误文案点名
   `importedHistory` / `remoteMirror`）；超时、网络中断等暂时性导入失败只跳过该条并计入
   `importFailures`，不得判成「版本过低」，也不得中止整轮方向（见「2026-10-16 第五轮修复」成因 C）。
   单条来源读取失败按 taskId 退避（60 s 起、指数翻倍、上限 10 min），退避期内不再重读但**仍计失败信号**，
   因此持续失败既不会每轮重撞、也不会被洗成 ready；手动重试先清退避。
7. **读之前就能判定的事不拖到读之后**：目标 scope 水位不前进的来源**不读消息**。
   同一口径的另一半是**读的量级也不拖到读之后**：`limit` 下推到存储查询，单次读取只取最新 N 条，
   而不是把整条会话取出再切片（「2026-10-16 第五轮修复」成因 A）。
   这是「空转轮 ≈ 满负荷轮」的修法，也是同一 workspace 同时只跑一轮的前提（见「第四轮修复」）。
   状态归属：轮次串行与退避表都是 Renderer 侧状态，所有权在 `useWorkspaceConversationMirror`
   这一个 hook 实例内（每工作区一份），不进 store、不跨窗口共享。
7. **不启动额外 runtime**：只处理用户正在使用的工作区；列举走任务列表（被动观察者），
   消息读取走既有 `session/messages`。冷会话由 CLI **只读 session store** 提供（不 resume、不物化 runtime，
   见「2026-10-15 冷读修复」），因此读取结果不再取决于「对端该会话是否活跃」。

### 事件顺序

`A` 连接 `B` 的远端工作区 `W`（`rpcReady` + 工作区行已展开 → 挂载镜像状态组件）

触发源：连接就绪 / 窗口（侧栏可见条数）变大 / 两个来源 scope 的
`workspace_task_list_changed`（防抖 1.2 s，且距上轮开始 ≥ 20 s）/ 手动重试。
**触发不等于开轮**：已有一轮在跑时只置 `pending`，当前轮结束后补跑一次（第四轮修复 2）。

```text
事件 → 防抖 (≥1.2s, 距上轮开始 ≥20s) ┬─ 无轮在跑 → 开轮
                                    └─ 有轮在跑 → pending=true（结束后补跑一轮，不再叠加）
轮：列目标 scope meta（水位表）
    └ 逐条来源：shouldReadMirrorSource? ── 否 → 跳过（不读消息，第四轮成因 A）
                                        └ 是 → 退避期内? ── 是 → 跳过 + 仍计失败信号
                                                          └ 否 → 读消息
                                                                 ├ 失败 → 记退避 + 计失败
                                                                 └ 成功 → 清退避 → 后置幂等判定 → 导入
```
```text
① 对端记录 → A 的视图 scope（会话落在 B 上）
  │   remote.modeTaskService.listTaskList({workspacePath: W})               ← 取 B 的本地 scope 行
  │     └ 过滤：带 identity 的行（对端身份）与 mirror- 前缀（副本）都跳过；取最新 windowSize 条
  │   remote.modeTaskService.listTaskList({workspacePath: W, identity: R})  ← 目标 scope 现有 meta
  │     └ shouldReadMirrorSource：origin.updatedAt > 目标 meta.updatedAt 才继续（未变则连消息都不读）
  │   remote.modeSessionService.readSessionMessages({W, sessionId}) → 提取文本 + 截断
  │   remote.modeSessionService.createSession({
  │       sessionId: mirror-<originSessionId>, persistence: "immediate",
  │       workspace: {workspacePath: W, workspaceIdentity: R},
  │       importedHistory: {source: "remoteMirror", title, createdAt, updatedAt, originSessionId, messages}})
  │     └ CLI：upsert 会话 → 清理旧导入消息 → 写消息/part(migrationSource) → app.resume()（可续聊）
  │
  └─ ② 本机记录 → B 的本地 scope（会话落在 B 上）
      来源 = remote.listTaskList({W, identity: R}) ← 合并
             base.listTaskList({W})               （A 同路径本地工程）
        └ 过滤：只跳过 mirror- 前缀（identity 行是本机自己发起的对话，必须保留）；取最新 windowSize 条
      消息回各自所在的机器读（视图 scope 的行在 B 上，本地 scope 的行在 A 上）
      remote.modeSessionService.createSession({sessionId: mirror-<originId>,
        workspace: {workspacePath: W},       ← 不带 identity = B 的本地 scope
        importedHistory: {source: "remoteMirror", ...}})
      → B 打开该工作区时看到 A 的对话，同样可续聊 / 可操作
```

### 错误处理（fail-soft，全部可读文案）

| 场景                                     | 行为                                                                 |
| ---------------------------------------- | -------------------------------------------------------------------- |
| 对端 CLI 不认识 `remoteMirror`（旧版本） | 视图标 unsupported +「对端版本过低，无法同步对话记录」，本轮不再重试；判定只认「载荷被对端 schema 拒收」（`-32602` / `-32601` / 文案点名导入字段） |
| 单条记录读取失败                         | 跳过该条 + warn，其余继续                                            |
| 单条导入失败（超时 / 网络等暂时性错误）  | 跳过该条 + warn + 计 `importFailures`；本轮 0 导入时报 degraded，**不判「版本过低」、不中止方向** |
| 来源列表一页装不下 windowSize 条可用来源 | 按可用条数逐级补页（30 → 60 → 120 …，上限 200），直到够或列表到底 |
| 单条来源连续读取失败                     | 按 taskId 退避（60 s → 10 min 封顶，成功清零），退避期内不重读、仍计失败信号 |
| 有记录可读但**本轮全部读取失败**         | 视图标 degraded + 重试入口（对端 CLI 无冷读能力时即此状态），不得伪装成 ready |
| 本机在该路径下无本地记录                 | ②方向只跳过这部分来源（另一个来源 scope 照常处理），不导入空内容      |
| 记录内容为空（全是工具调用 / 隐藏消息）  | 跳过该条（不导入空会话）                                             |
| 目标 meta 更新时间 ≥ 来源更新时间        | 跳过（无需台账即可幂等，重连不重复导入）                             |
| 超限                                     | 丢最旧消息 + `truncated`                                             |

## 验收场景

1. A 连接 B 的 `D:/proj`：A 的任务列表出现 B 的本地会话，且能**继续对话**（消息发到 B 执行）。
2. 上条会话可**置顶 / 归档 / 改名 / 删除**，删除后重连不复活（tasks-index tombstone）。
3. 该会话出现在**任务搜索**与 `@` 提及里（导入不写 migrationSource 到 task meta）。
4. **A 在远端工作区 `W` 发起的对话，B 打开 `W` 时也看得到**（②方向来源含本机视图 scope），
   同样可续聊 / 可操作。
5. 反复重连：不产生重复会话，也不丢失用户在镜像会话里续聊出的消息。
6. 对端为旧版本（CLI 不认 `remoteMirror`）→ 明确提示，连接与任务列表不受影响。
7. 断开远端连接后镜像会话不可见（属于对端机器），重新连接后仍在。
8. **连接之后**任一端新开 / 续聊的对话，另一端无需重连即可看到（工作区事件触发的防抖重同步）。
9. **窗口**：一轮至少同步每端最近 10 条（侧栏默认只可见 5 条也一样）；侧栏「展开更多」后窗口 +5，
   更旧的记录随之被同步，上限 20 条。
10. **排序**：镜像会话在侧栏按来源的**真实时间轴**排列（不是导入顺序），且与重启后冷启动的顺序一致；
    两端同一来源会话的创建 / 更新时间一致（CLI 透传 `updatedAt` 并回填活跃 record）。

## 测试

- 单测（shared）：`packages/shared/test/conversation-mirror.test.ts`——确定性 id 派生稳定、镜像前缀判定、
  ①方向过滤 identity 行 / ②方向保留 identity 行（两个谓词不同）、窗口取最新 N 条、
  窗口口径归一（可见 5 条 → 10 条；随可见条数增长并以 20 条封顶；未传 / NaN / 0 取下限）、
  多来源按 taskId 合并去重、消息提取（跳过隐藏 synthetic / 空文本）、
  有界化（会话数 / 消息数 / 文本长度 / 单记录字节预算 → `truncated`）、导入 payload 构造。
- 单测（ui）：`packages/ui/test/conversationMirrorSync.test.ts`——夹具按双机拓扑搭，远端机的本地 scope
  同时是①方向的来源与②方向的目标、视图 scope 是①的目标与②的来源，避免两边共用一份服务桩而掩盖方向接错。
  覆盖：两个方向各落一个 scope（视图带 identity / 反向不带）、②方向处理本机在远端工作区发起的对话、
  窗口至少 10 条且变大后更旧的记录才进来、目标不更旧不重导、mirror 前缀不被当新来源也不被再搬一次、
  对端不支持时整轮降级、空记录不导入、单条读取失败只跳过该条、本地工作区不做任何镜像。
  「第四轮修复」补：**水位不前进时连消息都不读**（断言读取次数为 0，这是空转轮的开销来源）、
  **退避期内不重读但本轮仍报 degraded**、**成功读取后清退避（下一轮恢复读取）**。
  「第五轮修复」补：**来源列表被镜像副本占满时逐级补页**（断言起始一页 = 30 且确有更大的补页请求、
  真实来源仍被导入）、**暂时性导入失败报 degraded 而不是 unsupported**、
  **`-32602` 拒收判 unsupported 且首个失败即中止该方向**。
- 单测（shared）：`packages/shared/test/conversation-mirror.test.ts` 另需覆盖 `shouldReadMirrorSource`
  与 `shouldImportMirrorRecord` **同判据**（同一 `updatedAt` 关系下两者结论一致）——这条等价性是
  「读之前判」能替代「读之后判」的全部依据。
- 单测（shared 协议）：`packages/shared/test/mode-protocol-*.test.ts` 断言 `file` part 的空 `url`
  能通过 `modeSessionMessagesResultSchema`（第四轮成因 C 的回归守卫：这条以前会让整条会话读不出来）。
- CLI 侧（`apps/mode-cli/packages/bootstrap/test/`）：
  `session-messages-cold-read.test.mts` 除冷读语义外，**必须**断言返回值通过
  `modeSessionMessagesResultSchema`（成因 0 的回归守卫——只断言文本内容就漏掉了形状错误，
  这正是它上一轮漏过 ZodError 的原因）；
  「第五轮修复」再补三条：冷读且无 `afterMessageId` 时 **`limit` 下推到 store**（断言 store 收到的入参
  就是 `{sessionID, limit}`）、带 `afterMessageId` 时**不下推**、会话带 conversation rewind 账本时**不下推**。
  `imported-history-timestamps.test.mts` 钉住 `remoteMirror` 透传 `createdAt/updatedAt`
  （这是镜像「已同步到哪一版」的水位，被压成 `createdAt` 会导致两端不一致 + 每轮重导），
  以及同一份时间线回填到活跃 record（`applyImportedHistoryTimelineToRecord`）——不回填则
  sessions-index `lastActivityAt` 落在导入那一刻，镜像按导入顺序排在侧栏最前。
  **仍缺**：`session/create` 带 `importedHistory.source="remoteMirror"` 端到端落盘为可续聊会话的用例。
- 单测（adapters）：`apps/mode-cli/packages/adapters/test/session-store-messages-limit.test.mts`——用真实
  sqlite session store 落 5 条带 part 的消息，钉住 `messages({ sessionID, limit })` 的语义：只取最新 N 条、
  与全量读的尾部**逐项相等（含 parts）**、limit 超总数返回全部、非法 limit（0 / 负数 / NaN / 非整数）
  退化成全量读。这条是「下推等价于 `slice(-N)`」这个前提的守卫——只在编排层用桩断言参数会被 SQL 写错蒙混过关。
- E2E（交互改动必需）：远端工作区连接后镜像会话进入任务列表并可续聊；两端各发起一条对话后对方可见。
- 验证：`pnpm typecheck`（另需 `pnpm --filter @mode/bootstrap typecheck`）、`pnpm lint`、
  `pnpm architecture:check --changed`，目标包测试按实际入口执行并如实报告。

## 不做

- **不复制工具调用 / 附件字节**：镜像只有角色 + 文本 + 时间戳（工具调用与附件留在来源机器的工作区）。
- **不做实时帧推送**：只在连接就绪 / 窗口变化 / 来源 scope 事件（防抖 + 20s 间隔下限）/ 手动刷新时同步。
- **不为远端工作区建本地离线副本**：归属固定在工作区所在机器（见上表）。
- **不做冲突合并**：同一来源记录按 `updatedAt` 单向覆盖导入消息（用户续聊消息不受影响）。
- 不含手机远控（`web-remote-replayable`）专项；不含子 agent / 工作流子会话。
- 不做工作区内落盘交换目录（`.mode-share` 式）。
