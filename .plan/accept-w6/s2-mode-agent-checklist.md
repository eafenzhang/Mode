# S2 mode-agent 翻面真机验收清单（spec host-ownership-switch.md）

门禁：**20 个真实任务全过**（spec 原文「打开开关后 20 个真实任务通过」）。
前置：真机构建 + `MODE_HOST_INPROCESS_CHANNELS=default,mode-agent`（S2 频道需显式
包含；整机走 default 集时 mode-agent 由本清单验收通过后并入）。
口径：适配器 88/100 请求面 + 15 事件面已接（2026-11-05）；automation 10 = 决策 4
`not-needed`（Renderer 分调两个 owner）；provider/account 2 = 决策 2 显式
`unsupported_method`（回滚 = `MODE_HOST_INPROCESS_CHANNELS=none`）。
记录：每任务一段「PASS/FAIL + 现象 + 频道日志引用」，写回 `report-<版本>.md`。

| # | 真实任务 | 覆盖方法 | 验收要点 |
| --- | --- | --- | --- |
| 1 | 新建会话并发首条 prompt | createSession / sendPrompt | 会话创建、回复流式返回；prompt 提交走 mode-agent 频道 |
| 2 | 会话列表与详情 | listSessions / readSession | 列表与详情字段与 Node Host 时代一致 |
| 3 | 会话消息与分页 | readSessionMessages / conversationRowsRangeV4 | 消息内容、rows 区间读取一致 |
| 4 | 续聊既有会话 | resumeSession / sendPrompt | 冷恢复不重置上下文；续聊正常 |
| 5 | 切模型 / 思考档 / 模式 | setModel / setThoughtLevel / setMode | 三开关即时生效且会话内保持 |
| 6 | 会话压缩 | compactSession | 压缩后上下文缩短、行为不回退 |
| 7 | Goal 会话 | goalSession | goal 设定/暂停/完成状态流转一致 |
| 8 | 会话事件流 | subscribeConversationV4 / unsubscribeConversationV4 / readSessionEvents | 事件到达顺序与去重一致；退订后不再推送 |
| 9 | sessions-index 订阅 | subscribeSessionsIndexV4 / resyncSessionsIndexV4 | 侧栏 live 增量与全量 resync 一致 |
| 10 | 附件四步 | attachmentBeginV4 / attachmentChunkV4 / attachmentCommitV4 / attachmentReadV4 | 上传/读取闭环；abort/preview 路径可用 |
| 11 | 会话附件读取 | conversationAttachmentReadV4 / conversationAttachmentStatV4 | 历史附件可读 |
| 12 | 用量面板 | getAppUsageStats / getTaskTokenUsage | 数字与 DB 直查一致 |
| 13 | 插件列表与总览 | listPlugins / getPluginsOverview / describePlugin | 清单、详情与 marketplace 来源一致 |
| 14 | 插件启停 | setPluginEnabled / configurePlugin / resetPluginConfig | 开关生效；config 变更与重置一致 |
| 15 | 插件安装 / 卸载 | installPlugin / uninstallPlugin / updatePlugin | 安装→启用→卸载闭环；进度事件（onDynamicPluginOperationProgress）到达 |
| 16 | marketplace 增删 | addPluginMarketplace / removePluginMarketplace / updatePluginMarketplace / restoreBuiltinPlugin | 市场增删改与内置恢复一致 |
| 17 | 工作流六法 | listSavedWorkflows / getSavedWorkflow / listSavedWorkflowRuns / updateSavedWorkflowMeta / deleteSavedWorkflow / moveSavedWorkflow | 全部 CRUD + 运行列表一致 |
| 18 | MCP 状态 | listMcpServerStatuses | 词源一致；mode 被拒返回稳定码 |
| 19 | workspace 配置与文件变更 | subscribeWorkspaceConfigV4 / conversationFileChangesV4 / conversationFileRewindPreviewV4 / resyncWorkspaceConfigV4 | 配置订阅与文件变更/回滚预览一致 |
| 20 | 会话收尾 | closeSession / disposeWorkspace / disposeAll | 会话关闭、workspace 释放、全量 dispose 无孤儿（进程核对） |

已知保真缺口（验收时如实记录现象，不因此判 FAIL）：
- sendPrompt 缺省时无 browserAmbientContext 主动采集（依赖浏览器控制面迁移；
  params 显式携带时正常透传）。
- createSession 的 provider 预同步（依赖 Node Host provider 编排迁移）。
