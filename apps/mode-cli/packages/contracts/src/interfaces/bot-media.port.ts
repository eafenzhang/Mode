/**
 * Bot 出站媒体端口。
 *
 * 只在 IM bot 回合内由 Host 注入：模型只表达“把某个文件发到当前会话”，
 * 目标（provider / botId / chatId）由 Host 从本轮 prompt 的 botDeliveryTarget 决定，
 * 路径白名单与大小限额由 Host 侧 botsService 统一校验。
 */
export interface BotMediaPort {
  sendMedia(input: BotMediaSendInput): Promise<BotMediaSendResult>;
}

export interface BotMediaSendInput {
  /** 工作区相对路径或绝对路径；Host 负责 realpath 与白名单校验。 */
  filePath: string;
  /** 可选的说明文字；平台不支持内联 caption 时由 provider 另发一条文本。 */
  caption?: string;
}

export interface BotMediaSendResult {
  filename: string;
  kind: "image" | "file";
}
