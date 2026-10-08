# 会话「正在输入」三点气泡

## 产品规则

- **触发**：当前轮次 `isRunning` 且**零助手内容**（assistant 行随内容到达才创建，所以等价于「消息已发出、回复首字未到」的空窗）。判据纯函数 `shouldShowTypingBubble`（`packages/ui/src/v4/conversationTypingBubble.ts`）。
- **呈现**：复用 `TurnChatLoadingSlot` 槽位（`ConversationTurnGroup.tsx`），空窗时渲染助手侧三点气泡（3 圆点逐个脉冲、末点品牌色，`motion-safe` 尊重减少动画偏好，`role="status"` + `chat.typing` i18n）。优先级：api 重试状态 > 气泡 > ChatLoading 兜底。
- **让位**：任一 assistant 行出现（回复开始）→ 判据翻 false → 槽位回到 ChatLoading；中断/等待授权由槽位既有 `eligible` 门禁兜住，不显示气泡。
- 不引入新协议字段：状态全部来自既有 turn 渲染单元（`unit.isRunning` / `assistantWorkRows` / `assistantTextRows`）。

## 验收场景

1. 发送消息后、首字到达前：时间线底部出现三点气泡（`data-testid="conversation-typing-bubble"`），无 ChatLoading。
2. 回复开始流式输出：气泡消失，ChatLoading/正文接管；中断与等待授权场景不出现气泡。
3. i18n 三语齐：`chat.typing` = 正在输入 / Typing / در حال تایپ。
4. 单测钉判据三态与三个槽位调用点的接线（`packages/services/test/conversation-typing-bubble.test.ts`）。

## 本轮验证状态

- 已验证：判据单测、槽位接线契约、typecheck/lint/services 全绿。
- **待实机**：dev 实例当前「没有可用模型」，无法真实触发一轮运行；配置模型后按场景 1/2 抓一次真实空窗即可闭环。
