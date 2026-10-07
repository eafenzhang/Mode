import type { BaseMessage, WSClient, WsFrame } from "@wecom/aibot-node-sdk";

/**
 * 企业微信连接的共享注册表。
 *
 * 由 channel runtime 在长连接建立/销毁时登记；provider 在出站发送、流式回复与
 * 附件下载时取用。拆成独立模块是为了让“连接生命周期”与“出站协议”各自独立演进。
 */
export interface WeComConnection {
  client: WSClient;
  /** 取该会话最近一次入站帧（流式回复/回复媒体需要透传 req_id）。 */
  getLastFrame(chatKey: string): WsFrame | null;
}

export interface WeComConnectionRegistry {
  getConnection(botId: string): WeComConnection | null;
  /** 仅供通道运行时登记连接；provider 通过 getConnection 使用。 */
  setConnection(botId: string, connection: WeComConnection): void;
  clearConnection(botId: string, connection: WeComConnection): void;
}

export function createWeComConnectionRegistry(): WeComConnectionRegistry {
  const connections = new Map<string, WeComConnection>();
  return {
    getConnection(botId: string) {
      return connections.get(botId) ?? null;
    },
    setConnection(botId, connection) {
      connections.set(botId, connection);
    },
    clearConnection(botId, connection) {
      if (connections.get(botId) === connection) {
        connections.delete(botId);
      }
    },
  };
}

/** 从入站消息解析会话键：群聊用 chatid，单聊用发送者 userid。 */
export function resolveWeComChatKey(message: Pick<BaseMessage, "chatid" | "from">): string {
  return message.chatid?.trim() || message.from.userid;
}
