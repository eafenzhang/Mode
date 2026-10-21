import type {
  BotActor,
  BotConfig,
  BotInboundAttachment,
  BotInboundMessage,
  BotOutboundMessage,
  BotProviderCallbackResult,
  Locale,
} from "@mode/shared";

export interface BotTypingTarget {
  providerUserId: string;
  providerMessageId?: string;
  providerContextToken?: string;
}

export interface BotProviderDownloadedAttachment {
  attachment: BotInboundAttachment;
  data: Uint8Array;
}

/** 出站媒体（服务层完成路径校验与大小上限后交 provider 上传）。 */
export interface BotOutboundMedia {
  kind: "image" | "file";
  filename: string;
  mimeType: string;
  data: Uint8Array;
  caption?: string;
}

export type BotStreamingReplyCardBlock =
  | {
      type: "message";
      text: string;
    }
  | {
      type: "tools";
      summaries: string[];
      title?: string;
      expanded?: boolean;
    };

export interface BotStreamingReplyCardState {
  providerUserId: string;
  locale?: Locale;
  blocks: BotStreamingReplyCardBlock[];
  status: "running" | "sealed" | "completed" | "error";
}

export interface BotStreamingReplyCardHandle {
  providerMessageId: string;
}

export type BotTransientInteractionCardHandle = BotStreamingReplyCardHandle;

export interface BotProviderAcknowledgeResult {
  handled?: boolean;
}

/**
 * 官方任务流生命周期，供传输型 provider 对齐 accepted→delivery→status 轮次收口。
 * `started` 表示已进入任务流（此时不应提前收口）；其余为终态/等待交互。
 */
export type BotTaskLifecyclePhase = "started" | "awaiting_input" | "completed" | "failed";

export interface BotProviderAdapter {
  test(bot: BotConfig): Promise<{ ok: boolean; message: string }>;
  resolveName?(bot: BotConfig): Promise<string | null>;
  syncCommands?(bot: BotConfig): Promise<void>;
  send(bot: BotConfig, message: BotOutboundMessage): Promise<void>;
  /**
   * 主动续期探测（context_expired 恢复链第一步，对齐 MyAgents「强刷票重试」模式）：
   * 对会话型平台（微信 iLink）主动拉一次会话配置，可能带回新 context_token；
   * 探测调用本身也可能在服务端重建会话状态。返回 undefined 表示没拿到新 token，
   * 探测失败也按 undefined 降级（交给既有恢复链继续）。
   */
  refreshContextToken?(
    bot: BotConfig,
    target: { providerUserId: string },
  ): Promise<string | undefined>;
  sendTyping?(bot: BotConfig, target: BotTypingTarget): Promise<void>;
  startTyping?(bot: BotConfig, target: BotTypingTarget): Promise<void>;
  stopTyping?(bot: BotConfig, target: BotTypingTarget): Promise<void>;
  resolveActorDisplayName?(bot: BotConfig, actor: BotActor): Promise<string | null>;
  acknowledgeCallback?(
    bot: BotConfig,
    payload: unknown,
    text?: string,
    message?: BotOutboundMessage,
    signal?: AbortSignal,
  ): Promise<BotProviderAcknowledgeResult | void>;
  /**
   * 该会话此刻是否具备流式写通道（企微必须拿到入站帧的 req_id 才能 replyStream）。
   * 返回 false 时服务层本回合直接走离散气泡，不把正文攒到终稿
   * （docs/specs/bot-reply-granularity.md「流式不可用时回退逐块气泡」）。
   * 未实现的 provider 视为「总是可写」，由 create 失败进入既有退避/熔断。
   */
  canWriteStreamingReply?(bot: BotConfig, targetProviderUserId: string): boolean;
  createStreamingReplyCard?(
    bot: BotConfig,
    state: BotStreamingReplyCardState,
    signal?: AbortSignal,
  ): Promise<BotStreamingReplyCardHandle | null>;
  updateStreamingReplyCard?(
    bot: BotConfig,
    handle: BotStreamingReplyCardHandle,
    state: BotStreamingReplyCardState,
    signal?: AbortSignal,
  ): Promise<void>;
  splitStreamingReplyCardStates?(state: BotStreamingReplyCardState): BotStreamingReplyCardState[];
  createTransientInteractionCard?(
    bot: BotConfig,
    message: BotOutboundMessage,
  ): Promise<BotTransientInteractionCardHandle | null>;
  updateTransientInteractionCard?(
    bot: BotConfig,
    handle: BotTransientInteractionCardHandle,
    message: BotOutboundMessage,
  ): Promise<void>;
  deleteTransientInteractionCard?(
    bot: BotConfig,
    handle: BotTransientInteractionCardHandle,
  ): Promise<void>;
  prepareCallbackPayload?(bot: BotConfig, payload: unknown): Promise<unknown>;
  handleCallbackResponse?(
    bot: BotConfig,
    payload: unknown,
  ): Promise<Pick<BotProviderCallbackResult, "responseBody" | "status"> | null>;
  downloadAttachment?(
    bot: BotConfig,
    attachment: BotInboundAttachment,
    actor?: BotActor,
  ): Promise<BotProviderDownloadedAttachment | null>;
  /**
   * 出站媒体（图片/文件）。未实现的 provider 视为"该通道不支持发送文件"。
   * target 的 providerUserId 已是最终投递目标（单聊 userid 或群 chatid）。
   */
  sendMedia?(bot: BotConfig, target: BotTypingTarget, media: BotOutboundMedia): Promise<void>;
  /**
   * 任务流生命周期通知（可选）。仅在 provider 需要把官方轮次映射成自有传输信号时实现，
   * 不改变任何业务状态。
   */
  notifyTaskLifecycle?(bot: BotConfig, actor: BotActor, phase: BotTaskLifecyclePhase): void;
  parseCallback(payload: unknown): BotInboundMessage[];
}
