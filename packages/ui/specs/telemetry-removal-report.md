# UI / Shared / Server 遥测清理报告

本轮删除 UI 的上报、操作追踪、TTFT、发送/打开耗时、错误上报、引导/登录/自动化埋点、购买漏斗及 WebView 上报上下文注入。业务回调直接执行，ErrorBoundary、反馈、logger、订阅恢复和队列确认保留。WebView 仍清理旧版本持久化的 report-context，避免历史数据继续被网页读取。

平台 reportTelemetryEvent、reportArmsCustomEvent、action trace、heap/TTFT 桥及相关 IPC 声明删除，Web/Desktop 的空适配器删除。preload 在前轮已清理，本轮扫描无残留、无需修改。server 不再设置 processResourceTelemetry。

connectTrigger 的类型移入 remoteWorkspaceConnection.ts；SessionCreateSource 复用仓库已有的 sessionCreateSource.ts；资源样本仅保留现有 client/services 协议仍使用的 lane 枚举到 processResourceSample.ts。均保持业务字段和值。

## 验证

- pnpm typecheck：通过，退出码 0。
- pnpm lint：通过，退出码 0；52 warnings，0 errors。
- pnpm architecture:check --changed：0 violations / 0 new。
- node --test packages/ui/tests/no-telemetry.test.mjs packages/desktop/tests/no-telemetry.test.mjs apps/mode-cli/tests/no-telemetry.test.mjs：11 通过，0 失败。
- git diff --check：通过。
- 63 个实际用户操作包装的业务回调已对照原源码核验，执行内容保留；设置页内部仅负责追踪的中转包装一并删除。
- 实际执行购买 WebView 注入脚本，验证 Zai/BigModel 凭据、JWT、主题、语言与 auth-ready 事件保留，不注入上报上下文。
- 未启动完整桌面/Web E2E；不能将上述静态和模块级验证视为完整交互验收。
- freshness 检查已尝试，因沙箱不允许写 .git/FETCH_HEAD 而无法 fetch。使用缓存 pnpm 10.33.2 执行仓库根脚本；可用 Node 为 24.16.0，mise.toml 固定的是 24.14.0。

## 保留的兼容命名

- services 的 telemetry-state.json / telemetry-state.lock：设备身份持久化与跨进程锁，供业务 X-Device-Mid 使用；与同机 CLI/旧版本共享，不删除用户身份文件。
- shared/mode-protocol-v4/telemetry.ts 的 ConversationTelemetryFact、v4/telemetry/event 以及 onDynamicConversationTelemetryFact：服务端任务活跃状态仍消费的既有协议事实。server 的本地订阅变量已改为 facts；不变更跨版本 wire name，不附带 UI 上报。
- shared/mode-protocol/index.ts 的 ZCodeMcpTelemetryEvent 等旧协议类型、mode-task-types-core.ts 的 skillMetadata：client/services 仍引用和解码，整体删除需另行迁移这些消费者。本轮已删除无人消费的 Host MCP/网络 telemetry 通道与 schema。
- shared/runtimeEnv.ts 的 isZCodeAgentTelemetryEnvKey：用于拒绝旧遥测环境变量继续进入 Agent，属于关闭遥测的过滤边界。

## 文件清单

列表仅记录本轮范围；工作区原有 Desktop/CLI/services 等改动未恢复、未提交。部分 shared 文件及 desktopPlatform.ts 与前轮改动重叠，列表不宣称其全部 diff 均由本轮产生。

### 删除（39）

- `packages/shared/src/node/nodeSelfResourceTelemetry.ts`
- `packages/shared/src/processResourceTelemetry.ts`
- `packages/shared/src/remoteUsageTelemetry.ts`
- `packages/shared/src/rendererActionTrace.ts`
- `packages/shared/src/sessionCreateTelemetry.ts`
- `packages/shared/src/telemetry.ts`
- `packages/shared/src/telemetryRedaction.ts`
- `packages/ui/src/hooks/usePlanIdentitySnapshot.ts`
- `packages/ui/src/lib/appTelemetry.ts`
- `packages/ui/src/lib/armsCustomEventObservability.ts`
- `packages/ui/src/lib/automationTelemetry.ts`
- `packages/ui/src/lib/chatErrorAttribution.ts`
- `packages/ui/src/lib/chatErrorAttributionEvidence.ts`
- `packages/ui/src/lib/chatErrorBannerTelemetry.ts`
- `packages/ui/src/lib/codingPlanFunnelTelemetry.ts`
- `packages/ui/src/lib/codingPlanOwnedEntryPlans.ts`
- `packages/ui/src/lib/launchToInputReport.ts`
- `packages/ui/src/lib/messageTelemetry.ts`
- `packages/ui/src/lib/offPeakTelemetry.ts`
- `packages/ui/src/lib/planUsageArmsTelemetry.ts`
- `packages/ui/src/lib/promptTemplateTelemetry.ts`
- `packages/ui/src/lib/providerTelemetryIdentity.ts`
- `packages/ui/src/lib/reactErrorArmsTelemetry.ts`
- `packages/ui/src/lib/sendFunnelArmsTelemetry.ts`
- `packages/ui/src/lib/sessionCreateTelemetry.ts`
- `packages/ui/src/lib/sessionOpenArmsTelemetry.ts`
- `packages/ui/src/lib/uiPerfArmsTelemetry.ts`
- `packages/ui/src/lib/userActionTelemetry.ts`
- `packages/ui/src/lib/userActionTraceCatalog.ts`
- `packages/ui/src/onboarding/useOnboardingTelemetry.ts`
- `packages/ui/src/settings/model-provider-section/oauthActions.ts`
- `packages/ui/src/v4/telemetry/ConversationTelemetryAttachment.tsx`
- `packages/ui/src/v4/telemetry/conversationPromptTelemetry.ts`
- `packages/ui/src/v4/telemetry/conversationTelemetrySupervisor.ts`
- `packages/ui/src/v4/telemetry/localTtftFacts.ts`
- `packages/ui/src/v4/telemetry/localTtftObserver.ts`
- `packages/ui/src/v4/telemetry/localTtftRecord.ts`
- `packages/ui/src/v4/telemetry/useSessionOpenArmsTelemetry.ts`
- `packages/ui/src/v4/telemetry/useSessionSubscriptionErrorTelemetry.ts`

### 修改（76）

- `packages/desktop/src/renderer/src/desktopPlatform.ts`
- `packages/server/src/entry-stdio.ts`
- `packages/server/src/http.ts`
- `packages/shared/src/automation-types.ts`
- `packages/shared/src/channels.ts`
- `packages/shared/src/env.ts`
- `packages/shared/src/index.ts`
- `packages/shared/src/node.ts`
- `packages/shared/src/official-glm-model-id.ts`
- `packages/shared/src/platform.ts`
- `packages/shared/src/server-remote.ts`
- `packages/shared/src/task-realtime-core.ts`
- `packages/shared/src/validation.ts`
- `packages/ui/src/App.tsx`
- `packages/ui/src/ChatEmptyState.tsx`
- `packages/ui/src/EmbeddedBrowserPaneParts.tsx`
- `packages/ui/src/ErrorBoundary.tsx`
- `packages/ui/src/GitActionMenu.tsx`
- `packages/ui/src/LexicalChatInput.tsx`
- `packages/ui/src/NewTaskButtonGroup.tsx`
- `packages/ui/src/Root.tsx`
- `packages/ui/src/SSHDialog.tsx`
- `packages/ui/src/SettingsPage.tsx`
- `packages/ui/src/TaskListItem.tsx`
- `packages/ui/src/WorkspaceSidebar.tsx`
- `packages/ui/src/WorkspaceSidebarFooterUsageSummary.tsx`
- `packages/ui/src/WorkspaceTerminalToggleButton.tsx`
- `packages/ui/src/app-shell/SelectionSideChatPane.tsx`
- `packages/ui/src/app-shell/SubagentSessionSidePane.tsx`
- `packages/ui/src/app-shell/WorkflowActorSessionSidePane.tsx`
- `packages/ui/src/hooks/useCodingPlanEntryPlanList.ts`
- `packages/ui/src/hooks/useOAuth.ts`
- `packages/ui/src/index.ts`
- `packages/ui/src/lib/memoryDiagnostics.ts`
- `packages/ui/src/onboarding/OccupationOnboarding.tsx`
- `packages/ui/src/root/RootWorkspaceContent.tsx`
- `packages/ui/src/root/useRootOAuthEffects.ts`
- `packages/ui/src/root/useRootWorkspaceActions.ts`
- `packages/ui/src/settings/AutomationEditView.tsx`
- `packages/ui/src/settings/AutomationsSection.tsx`
- `packages/ui/src/settings/BrowserSettingsSection.tsx`
- `packages/ui/src/settings/CodingPlanEmbeddedWebviewDialog.tsx`
- `packages/ui/src/settings/CodingPlanUpgradeDialog.tsx`
- `packages/ui/src/settings/CodingPlanUpgradeDialogProvider.tsx`
- `packages/ui/src/settings/McpServerList.tsx`
- `packages/ui/src/settings/MemorySettingsSection.tsx`
- `packages/ui/src/settings/ModelProviderSection.tsx`
- `packages/ui/src/settings/PluginStoreCard.tsx`
- `packages/ui/src/settings/codingPlanUpgradeLoginRecovery.ts`
- `packages/ui/src/settings/model-provider-section/Detail.tsx`
- `packages/ui/src/settings/model-provider-section/StatusCards.tsx`
- `packages/ui/src/settings/model-provider-section/codingPlanEmbeddedWebview.ts`
- `packages/ui/src/store/offPeakTaskStore.ts`
- `packages/ui/src/v4/ConversationAgentToolCallRow.tsx`
- `packages/ui/src/v4/ConversationComposer.tsx`
- `packages/ui/src/v4/ConversationDraftSuggestedPromptsContainer.tsx`
- `packages/ui/src/v4/ConversationHeader.tsx`
- `packages/ui/src/v4/ConversationQueuePanel.tsx`
- `packages/ui/src/v4/ConversationRowView.tsx`
- `packages/ui/src/v4/ConversationTimeline.tsx`
- `packages/ui/src/v4/SessionPane.tsx`
- `packages/ui/src/v4/V4ChatPane.tsx`
- `packages/ui/src/v4/V4ConversationContext.tsx`
- `packages/ui/src/v4/V4UserInputDialog.tsx`
- `packages/ui/src/v4/V4WorkspaceChatArea.tsx`
- `packages/ui/src/v4/WorkbenchPane.tsx`
- `packages/ui/src/v4/WorkspaceHookPendingBanner.tsx`
- `packages/ui/src/v4/agentConversationTransport.ts`
- `packages/ui/src/v4/composer/V4ComposerToolbar.tsx`
- `packages/ui/src/v4/conversationProjectionStore.ts`
- `packages/ui/src/v4/sessionDataLayer.ts`
- `packages/ui/src/v4/workspaceConnectionRegistry.ts`
- `packages/web/src/main.tsx`
- `packages/mode-server-cli/src/server-core/http.ts`
- `packages/mode-server-cli/src/server-core/taskActivityTracker.ts`
- `packages/desktop/specs/no-telemetry.md`

### 新增（5）

- `packages/shared/src/remoteWorkspaceConnection.ts`
- `packages/shared/src/processResourceSample.ts`
- `packages/ui/specs/no-telemetry.md`
- `packages/ui/tests/no-telemetry.test.mjs`
- `packages/ui/specs/telemetry-removal-report.md`
