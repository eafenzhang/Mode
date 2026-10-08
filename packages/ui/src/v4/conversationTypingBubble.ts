/**
 * 会话「正在输入」三点气泡的判据。
 *
 * 消息发出后到回复首字到达前，助手行还没有任何一条（行随内容到达才创建），这段空窗
 * 用气泡表示「正在输入」；回复一到（任一 assistant 行出现）就切回 ChatLoading 或正文。
 * 等待授权/回答、api 重试等态由 TurnChatLoadingSlot 的 eligible / retry 门禁兜住，
 * 这里只回答一件事：本轮在跑，且还没有任何助手内容。
 */
export function shouldShowTypingBubble(input: {
  isRunning: boolean;
  assistantRowCount: number;
}): boolean {
  return input.isRunning && input.assistantRowCount === 0;
}
