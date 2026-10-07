import type { Locale } from "@zcode/shared";

export type BotMessageLocale = Extract<Locale, "zh-CN" | "en-US">;

type MessageValues = Record<string, string | number | undefined>;

const DEFAULT_BOT_MESSAGE_LOCALE: BotMessageLocale = "zh-CN";

const messages = {
  "zh-CN": {
    botDisabled: "当前 bot 未启用。",
    privateChatOnly: "Bots 暂不支持群聊，请在私聊中使用。",
    bindPrivateOnly: "Bots 只允许在私聊中绑定。",
    userNotBound:
      "该机器人尚未绑定。请在 Mode 里生成绑定码后发送 **/bind <绑定码>**（绑定码 5 分钟内有效）。",
    commandNotAllowed: "当前 bot 未启用这个命令。",
    noWorkspaceAllowed: "没有可用 workspace，请先在 Bots 设置里允许 workspace。",
    workspaceOutOfScope: "当前聊天上下文的 workspace 已不在授权范围内，请重新选择 **/项目**。",
    bindCodeInvalid: "绑定码无效或已过期，请在 Mode 界面重新生成。",
    bindBotMissing: "绑定失败：bot 不存在。",
    weixinActivatedWelcome: "微信 Bot 已激活。发送 **/帮助** 查看命令，或直接描述你要做的事。",
    helpTitle: "Mode 机器人命令：",
    helpHelp: "**/帮助** — 查看这份说明",
    helpBind: "**/bind <code>** — 绑定当前聊天",
    helpStatus: "**/状态** — 查看工作区、模型和任务状态",
    helpNew: "/新建 或 /clear — 开始新的任务草稿",
    helpWorkspace: "**/项目** — 切换工作区",
    helpModel: "**/模型** — 切换模型",
    helpMode: "**/模式** — 切换运行模式",
    helpThoughtLevel: "**/思考** — 切换思考级别",
    helpReply: "**/回复** — 切换回复详细程度",
    desktopPromptMirror: "🖥 桌面端输入：{text}",
    helpSend: "**/send <路径>** — 把工作区内的文件发到当前会话（可加 `| 说明`）",
    sendMediaSent: "已发送：{file}",
    sendMediaFailed: "发送文件失败：{message}",
    sendMediaMissing: "用法：**/send <路径>**，路径需在当前工作区内（或系统临时目录）。",
    groupCommandUnsupported: "群聊中仅支持对话与 /help、/stop；管理命令请在私聊使用。",
    groupPromptDefaultBotName: "助手",
    groupPromptHeader:
      "[群聊信息] 你正在「{group}」{platform}群聊中。你的名字是「{botName}」。",
    groupPromptActivationMention: "激活模式：仅 @提及（只有被 @ 或被回复时才会收到消息）。",
    groupPromptActivationAlways:
      "激活模式：全部消息（你会收到群里所有消息，包括不是发给你的）。",
    groupPromptSenderNote:
      "群内不同人的消息会以 [from: 名字 时间] 标注发送者；你的回复会自动发送到群里，直接回复即可。",
    groupPromptReplyRules:
      "[回复规则] 你必须非常克制，大多数消息不需要你回复。仅在以下情况回复：1. 消息明确 @你（即使同时 @了其他人）；2. 消息回复了你之前的消息；3. 有人直接向你提问或请求帮助；4. 你确信能提供明确价值的信息。以下情况必须保持沉默：没有 @你、只 @了其他人或其他机器人；与你无关的闲聊；你不确定是否该回复时。判断是否 @了你：看 [本条消息 @了你，你需要回复] / [本条消息未 @你] 标记，而不是正文里的 @用户名。不需要回复时，只回复 <NO_REPLY>，不要输出其他任何内容。",
    groupPromptFromLine: "[from: {sender} {time}]",
    groupPromptUnknownSender: "未知成员",
    groupPromptMentionedMarker: "[本条消息 @了你，你需要回复]",
    groupPromptNotMentionedMarker: "[本条消息未 @你]",
    webhookSecretInvalid: "Webhook secret 校验失败。",
    // Bugfix: 这条错误由通用 provider callback 处理路径触发，微信/飞书失败时不能误显示 Telegram。
    callbackFailed: "处理机器人回调失败：{message}",
    sessionExpiredNewTaskHint:
      "当前任务会话已失效，可能是任务已被清理或机器人消息已过期。请发送 **/new task** 创建新任务后再继续。",
    deletedTaskReplaced:
      "原任务已删除，已为你新建任务。本条消息将在新任务中处理，不会继承原任务的对话上下文。",
    received: "已收到。",
    attachmentOnlyPrompt: "请查看附件并根据内容协助我。",
    attachmentRejected: "附件处理失败：{message}",
    attachmentDownloadUnavailable:
      "无法下载附件。文件可能已过期、已撤回，或机器人没有读取权限。请重新发送附件后再试。",
    attachmentTooLarge: "附件超过 5MB，请压缩后重新发送。",
    selectionCancelled: "已取消。",
    selectionCancelOption: "取消",
    selectionTextHint: "回复数字选择，0 取消。",
    selectionTextHintNoCancel: "回复数字选择。",
    // AstrBot 桥接的 canonical 文本提示：纯文本渠道需要把「对应命令」一起告诉用户。
    selectionCommandHint: "回复 {command} <序号> 进行选择。",
    permissionSelectionHint: "回复 /permission <序号>，或直接发送上方对应命令。",
    elicitationReplyHint:
      "回复 /elicitation {token} <序号> 进行选择；多选完成后回复 /elicitation {token} submit。",
    newTaskDraft: "已进入 {workspacePath} 的新任务草稿。",
    workspaceSelectTitle: "当前 workspace {workspace}\n选择 workspace",
    workspaceMissing: "未找到可用 workspace。",
    modelSelectTitle: "选择 model",
    modelProviderSelectTitle: "当前模型 {model}\n选择模型供应商",
    modelModelSelectTitle: "当前模型 {model}\n选择模型",
    modelMissing: "未找到 model。",
    sessionModelUnavailable: "当前会话的模型选择不可用，请使用 /model 重新选择。原选择已保留。",
    modeSelectTitle: "当前模式 {mode}\n选择模式",
    modeMissing: "未找到模式。",
    modeChanged: "当前任务模式已切换为 {mode}。",
    modeLocked: "机器人已锁定 **yolo** 运行模式，无法切换。",
    thoughtLevelSelectTitle: "当前思考级别 {level}\n选择思考级别",
    thoughtLevelMissing: "当前模型不支持思考级别。",
    thoughtLevelChanged: "当前任务思考级别已切换为 {level}。",
    modelProviderMissing: "未找到模型供应商。",
    modelChanged: "当前任务 model 已切换为 {model}。",
    taskMissing: "未找到任务。",
    taskChanged: "已切换到任务：{title}",
    noActiveTask: "当前没有 active task。",
    permissionExpired: "权限请求已过期，请在 Mode 界面中处理。",
    permissionHandled: "权限请求已处理。",
    permissionDenied: "已拒绝权限请求。",
    permissionSubmitted: "已提交权限响应。",
    elicitationExpired: "问答请求已过期，请在 Mode 界面中处理。",
    elicitationHandled: "问答请求已处理。",
    elicitationSubmitted: "已提交问答响应。",
    elicitationCancelled: "已取消问答请求。",
    elicitationCustomOption: "自定义回答",
    elicitationCustomPlaceholder: "请输入自定义回答",
    elicitationQuestionTitle: "提问",
    planApprovalTitle: "请审阅此实施计划。",
    planApprovalHeader: "实施计划",
    planApprovalApprove: "批准",
    planApprovalApproveDescription: "退出计划模式并开始实施。",
    elicitationCancelledCard: "✅ 问答已取消",
    elicitationSubmitOption: "完成",
    elicitationSkipOption: "跳过",
    elicitationMultiSelectHint: "可多选；再次选择会取消，选择“完成”提交。",
    elicitationTextHint: "也可以直接回复文本作为自定义答案。",
    statusWorkspace: "工作区",
    statusModel: "模型",
    statusTask: "任务",
    statusState: "状态",
    statusWorked: "已工作",
    statusProgress: "进展",
    statusDraft: "草稿",
    statusRemoteDisconnected: "远端未连接",
    statusCancelled: "已取消",
    statusStopped: "已停止",
    streamingStatusRunning: "⏳ 运行中",
    streamingStatusCompleted: "✅ 已完成",
    streamingStatusFailed: "失败",
    streamingWorking: "正在处理...",
    taskCompleted: "任务已完成。",
    heartbeatTaskTitle: "心跳检查",
    streamingToolSummaries: "工具摘要",
    stopSubmitted: "已停止当前任务生成。",
    unknownCommand: "未知命令：**/{command}**",
    taskFailed: "任务失败：{message}",
    taskRunning: "当前任务正在运行，稍后再试，或使用 **/停止** 停止当前任务。",
    taskQueued: "当前任务正在运行，消息已排队（第 {position} 位），任务完成后自动执行。",
    taskQueuedDropped: "注意：队列已满，最早的一条排队消息被丢弃。",
    workspacePinned:
      "该 bot 已在 Mode 中绑定工作区，只能在与绑定工作区之间切换；请先在 Mode 的 Bot 设置里解绑。",
    taskSelectTitle: "当前任务 {task}\n选择任务",
    noHistoryTasks: "当前 workspace 没有历史任务。",
    remoteDisconnected:
      "当前远端项目 {workspacePath} 未连接。请先发送 **/重连**，连接恢复后再重试。上一条请求未执行。",
    remoteDisconnectedStatus: "当前远端项目 {workspacePath} 未连接。请发送 **/重连** 恢复连接。",
    remoteWorkspaceSelectedDisconnected:
      "已切换到远端项目 {workspacePath}，但当前未连接。请先发送 **/重连** 后再执行任务。",
    remoteReconnectStarting: "当前远端项目 {workspacePath} 未连接，正在为你重连...",
    remoteReconnectFailed: "当前远端项目 {workspacePath} 重连失败：{message}\n上一条请求没有执行。",
    remoteReconnectUnavailable:
      "当前远端项目 {workspacePath} 未连接，但机器人无法访问远端重连服务。请先在 Mode 打开该远端项目后重试。",
    remoteReconnectLocal: "当前 workspace 是本地项目，不需要重连。发送 **/项目** 可切换远端项目。",
    remoteReconnectAlreadyConnected: "当前远端项目 {workspacePath} 已连接。",
    replySelectTitle: "当前第三方回复颗粒度 {mode}\n选择第三方回复颗粒度",
    replyMissing: "未找到回复颗粒度。",
    replyChanged: "第三方回复颗粒度已切换为 {mode}。",
  },
  "en-US": {
    botDisabled: "This bot is not enabled.",
    privateChatOnly: "Bots do not support group chats yet. Please use a private chat.",
    bindPrivateOnly: "Bots can only bind in a private chat.",
    userNotBound:
      "This bot is not bound. Generate a bind code in the Mode UI (valid for 5 minutes), then send **/bind <code>** here.",
    commandNotAllowed: "This command is disabled for the current bot.",
    noWorkspaceAllowed: "No workspace is available. Allow a workspace in Bots settings first.",
    workspaceOutOfScope:
      "The workspace in this chat is no longer authorized. Please select **/workspace** again.",
    bindCodeInvalid: "The bind code is invalid or expired. Generate a new one in the Mode UI.",
    bindBotMissing: "Bind failed: bot does not exist.",
    weixinActivatedWelcome:
      "Weixin bot is active. Send **/help** to see commands, or describe what you want to do.",
    helpTitle: "Mode bot commands:",
    helpHelp: "**/help** — Show this guide",
    helpBind: "**/bind <code>** — Bind this chat",
    helpStatus: "**/status** — Show workspace, model, and task status",
    helpNew: "/new or /clear — Start a new task draft",
    helpWorkspace: "**/project** — Switch workspace",
    helpModel: "**/model** — Switch model",
    helpMode: "**/mode** — Switch run mode",
    helpThoughtLevel: "**/think** — Switch thought level",
    helpReply: "**/reply** — Switch reply detail",
    desktopPromptMirror: "🖥 Desktop input: {text}",
    helpSend: "**/send <path>** — Send a file from the workspace to this chat (optional `| caption`)",
    sendMediaSent: "Sent: {file}",
    sendMediaFailed: "Failed to send file: {message}",
    sendMediaMissing: "Usage: **/send <path>**. The path must be inside the current workspace (or the system temp dir).",
    groupCommandUnsupported:
      "Group chats only support conversation, /help and /stop. Use management commands in a private chat.",
    groupPromptDefaultBotName: "Assistant",
    groupPromptHeader:
      "[Group chat] You are in the {platform} group \"{group}\". Your name is \"{botName}\".",
    groupPromptActivationMention:
      "Activation mode: mention only (you receive a message only when mentioned or replied to).",
    groupPromptActivationAlways:
      "Activation mode: all messages (you receive every group message, including ones not addressed to you).",
    groupPromptSenderNote:
      "Messages from different members are tagged as [from: name time]. Your reply is sent to the group directly.",
    groupPromptReplyRules:
      "[Reply rules] Stay very restrained; most messages do not need a reply. Reply only when: 1. the message explicitly mentions you (even if it also mentions others); 2. the message replies to your earlier message; 3. someone asks you a question or requests help; 4. you are confident you add clear value. Stay silent when: you are not mentioned and only others are; the chat is unrelated small talk; you are unsure. Judge mentions by the [This message mentions you - you must reply] / [This message does not mention you] marker, not by @usernames in the text. When no reply is needed, output only <NO_REPLY> and nothing else.",
    groupPromptFromLine: "[from: {sender} {time}]",
    groupPromptUnknownSender: "unknown member",
    groupPromptMentionedMarker: "[This message mentions you - you must reply]",
    groupPromptNotMentionedMarker: "[This message does not mention you]",
    webhookSecretInvalid: "Webhook secret verification failed.",
    callbackFailed: "Failed to process bot callback: {message}",
    sessionExpiredNewTaskHint:
      "The current task session has expired. It may have been cleaned up, or this bot message is stale. Send **/new task** to create a new task and continue.",
    deletedTaskReplaced:
      "The previous task was deleted, so I created a new task for you. This message will be processed in the new task without the previous conversation history.",
    received: "Received.",
    attachmentOnlyPrompt: "Please review the attachment and help based on its content.",
    attachmentRejected: "Failed to process attachment: {message}",
    attachmentDownloadUnavailable:
      "Could not download the attachment. The file may have expired, been removed, or the bot may not have permission to read it. Please send the attachment again and try once more.",
    attachmentTooLarge: "The attachment exceeds 5MB. Compress it and send it again.",
    selectionCancelled: "Cancelled.",
    selectionCancelOption: "Cancel",
    selectionTextHint: "Reply with a number to choose, or 0 to cancel.",
    selectionTextHintNoCancel: "Reply with a number to choose.",
    // Canonical-text hints for the AstrBot bridge: text-only channels must also show the command.
    selectionCommandHint: "Reply with {command} <number> to choose.",
    permissionSelectionHint: "Reply with /permission <number>, or send the command above.",
    elicitationReplyHint:
      "Reply with /elicitation {token} <number>; send /elicitation {token} submit when a multi-select is done.",
    newTaskDraft: "Entered a new task draft in {workspacePath}.",
    workspaceSelectTitle: "Current workspace {workspace}\nSelect workspace",
    workspaceMissing: "No available workspace found.",
    modelSelectTitle: "Select model",
    modelProviderSelectTitle: "Current model {model}\nSelect model provider",
    modelModelSelectTitle: "Current model {model}\nSelect model",
    modelMissing: "Model not found.",
    sessionModelUnavailable:
      "The session's model selection is unavailable. Use /model to choose again. Your saved selection has been preserved.",
    modeSelectTitle: "Current mode {mode}\nSelect mode",
    modeMissing: "Mode option not found.",
    modeChanged: "Current task mode changed to {mode}.",
    modeLocked: "This bot is locked to **yolo** run mode and cannot be switched.",
    thoughtLevelSelectTitle: "Current thought level {level}\nSelect thought level",
    thoughtLevelMissing: "The current model does not support thought level.",
    thoughtLevelChanged: "Current task thought level changed to {level}.",
    modelProviderMissing: "Model provider not found.",
    modelChanged: "Current task model changed to {model}.",
    taskMissing: "Task not found.",
    taskChanged: "Switched to task: {title}",
    noActiveTask: "There is no active task.",
    permissionExpired: "This permission request has expired. Please handle it in the Mode UI.",
    permissionHandled: "Permission request has already been handled.",
    permissionDenied: "Permission request denied.",
    permissionSubmitted: "Permission response submitted.",
    elicitationExpired: "This question request has expired. Please handle it in the Mode UI.",
    elicitationHandled: "Question request has already been handled.",
    elicitationSubmitted: "Question response submitted.",
    elicitationCancelled: "Question request cancelled.",
    elicitationCustomOption: "Custom answer",
    elicitationCustomPlaceholder: "Enter a custom answer",
    elicitationQuestionTitle: "Question",
    planApprovalTitle: "Review this implementation plan.",
    planApprovalHeader: "Implementation plan",
    planApprovalApprove: "Approve",
    planApprovalApproveDescription: "Exit plan mode and start implementation.",
    elicitationCancelledCard: "✅ Questions cancelled",
    elicitationSubmitOption: "Done",
    elicitationSkipOption: "Skip",
    elicitationMultiSelectHint:
      "You can select multiple options; select again to remove, then choose Done.",
    elicitationTextHint: "You can also reply with text as a custom answer.",
    statusWorkspace: "Workspace",
    statusModel: "Model",
    statusTask: "Task",
    statusState: "State",
    statusWorked: "Worked",
    statusProgress: "Progress",
    statusDraft: "draft",
    statusRemoteDisconnected: "remote disconnected",
    statusCancelled: "cancelled",
    statusStopped: "stopped",
    streamingStatusRunning: "Running",
    streamingStatusCompleted: "Completed",
    streamingStatusFailed: "Failed",
    streamingWorking: "Working...",
    taskCompleted: "Task completed.",
    heartbeatTaskTitle: "Heartbeat check",
    streamingToolSummaries: "Tool summaries",
    stopSubmitted: "Current task generation stopped.",
    unknownCommand: "Unknown command: **/{command}**",
    taskFailed: "Task failed: {message}",
    taskRunning:
      "The current task is still running. Try again later, or use **/stop** to stop the current task.",
    taskQueued:
      "The current task is still running. Your message is queued (position {position}) and will run automatically when the task finishes.",
    taskQueuedDropped:
      "Note: the queue is full, so the oldest queued message was dropped.",
    workspacePinned:
      "This bot is workspace-bound in Mode and can only switch between its bound workspaces. Unbind it in the Mode bot settings first.",
    taskSelectTitle: "Current task {task}\nSelect task",
    noHistoryTasks: "There are no history tasks in the current workspace.",
    remoteDisconnected:
      "The remote workspace {workspacePath} is not connected. Send **/reconnect** first, then try again. The previous request was not executed.",
    remoteDisconnectedStatus:
      "The remote workspace {workspacePath} is not connected. Send **/reconnect** to restore the connection.",
    remoteWorkspaceSelectedDisconnected:
      "Switched to {workspacePath}, but the remote workspace is not connected. Send **/reconnect** before running tasks.",
    remoteReconnectStarting:
      "The remote workspace {workspacePath} is not connected. Reconnecting now...",
    remoteReconnectFailed:
      "Remote workspace {workspacePath} reconnect failed: {message}\nThe previous request was not executed.",
    remoteReconnectUnavailable:
      "The remote workspace {workspacePath} is not connected, but the bot cannot access the remote reconnect service. Open this remote project in Mode and try again.",
    remoteReconnectLocal:
      "The current workspace is local and does not need reconnecting. Send **/workspace** to switch to a remote project.",
    remoteReconnectAlreadyConnected: "The remote workspace {workspacePath} is connected.",
    replySelectTitle: "Current third-party reply detail {mode}\nSelect third-party reply detail",
    replyMissing: "Reply detail option not found.",
    replyChanged: "Third-party reply detail changed to {mode}.",
  },
} as const;

export type BotMessageId = keyof (typeof messages)[BotMessageLocale];

export function normalizeBotMessageLocale(locale: Locale | undefined): BotMessageLocale {
  // bot 文案只有 zh/en 两份；fa-IR 等其他界面语言跟随英文，undefined 保持 zh 默认。
  if (locale === "fa-IR") return "en-US";
  return locale === "en-US" ? "en-US" : DEFAULT_BOT_MESSAGE_LOCALE;
}

export function formatBotMessage(
  locale: Locale | undefined,
  id: BotMessageId,
  values: MessageValues = {},
): string {
  let message: string = messages[normalizeBotMessageLocale(locale)][id];
  for (const [key, value] of Object.entries(values)) {
    message = message.replaceAll(`{${key}}`, String(value ?? ""));
  }
  return message;
}
