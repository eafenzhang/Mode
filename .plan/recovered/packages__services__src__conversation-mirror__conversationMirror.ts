import type {
  ConversationMirrorIdentity,
  ConversationMirrorPublishResult,
  ConversationMirrorReadResult,
  ConversationMirrorSnapshot,
  ConversationMirrorWorkspaceRef,
} from "@mode/shared";
import { ServiceChannels } from "@mode/shared";
import { createServiceDescriptor } from "../descriptors.js";

/**
 * 远程工作区对话记录镜像服务（见 docs/specs/remote-workspace-conversation-mirror.md）。
 *
 * 服务层只做**存储**：接收对端发布的快照、按工作区路径读取全部快照。它不读取会话内容、
 * 不启动任何 runtime——快照由拥有会话的那台机器在自己的 UI 层构建后发布。这样镜像不会
 * 成为第二份会话权威，也不会因为「谁来读」而拉起对端 runtime。
 */
export interface IConversationMirrorService {
  /** 本机镜像身份：读取方用它剔除自己的快照（防回声），无需额外握手。 */
  getMirrorIdentity(): Promise<ConversationMirrorIdentity>;
  /**
   * 对端发布通道：接收对端的一份快照，按 origin 覆盖式落盘（幂等）。
   * 拒绝 origin 等于本机的快照——对端转发本机记录属于回声，不该进本机镜像目录。
   */
  publishSnapshot(params: {
    snapshot: ConversationMirrorSnapshot;
  }): Promise<ConversationMirrorPublishResult>;
  /**
   * 本机自快照通道：把本机自己的记录写进本机镜像目录，供对端连接时经 RPC 读取。
   * 只接受 origin 等于本机的快照（对端不能用这个入口伪造「本机记录」）。
   */
  writeLocalSnapshot(params: {
    snapshot: ConversationMirrorSnapshot;
  }): Promise<ConversationMirrorPublishResult>;
  /** 读取某工作区路径下的全部快照（含本机自身快照，调用方自行按 origin 过滤）。 */
  readWorkspaceSnapshots(
    params: ConversationMirrorWorkspaceRef,
  ): Promise<ConversationMirrorReadResult>;
}

export const IConversationMirrorService = createServiceDescriptor<IConversationMirrorService>(
  ServiceChannels.ConversationMirror,
);
