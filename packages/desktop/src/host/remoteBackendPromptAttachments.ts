import { Emitter } from "@mode/rpc";
import type { IRemoteBackend } from "@mode/server/remote";
import type {
  IPromptAttachmentTransferService,
  PromptAttachmentTransferProgress,
} from "@mode/services";
import type { ModePromptAttachment } from "@mode/shared";

import { materializeRemotePromptAttachments } from "./remotePromptAttachments.js";

/**
 * SSH/WSL/Docker 的附件暂存：没有 LAN 的 artifact 字节通道，直接用 backend.upload
 * 把本机文件送到远端私有临时目录；返回的 ref 是远端路径，renderer 不感知差异。
 */
export function createRemoteBackendPromptAttachmentTransferService(deps: {
  backend: Pick<IRemoteBackend, "exec" | "upload">;
}): IPromptAttachmentTransferService {
  const emitters = new Map<string, Emitter<PromptAttachmentTransferProgress>>();

  const getEmitter = (operationId: string) => {
    const existing = emitters.get(operationId);
    if (existing) {
      return existing;
    }
    const emitter = new Emitter<PromptAttachmentTransferProgress>({
      onDidRemoveLastListener: () => {
        emitters.delete(operationId);
        emitter.dispose();
      },
    });
    emitters.set(operationId, emitter);
    return emitter;
  };

  return {
    async stage(params) {
      const attachment: ModePromptAttachment = {
        kind: "file",
        filename: params.fileName,
        mimeType: params.mime,
        sizeBytes: params.sizeBytes ?? 0,
        localPath: params.localPath,
      };
      const result = await materializeRemotePromptAttachments(
        {
          taskId: params.sessionId,
          content: "",
          traceId: params.operationId,
          attachments: [attachment],
        },
        { backend: deps.backend },
      );
      const ref = result.attachments?.[0]?.localPath ?? params.localPath;
      const bytes =
        typeof params.sizeBytes === "number" && params.sizeBytes > 0 ? params.sizeBytes : 0;
      getEmitter(params.operationId).fire({
        operationId: params.operationId,
        phase: "complete",
        uploadedBytes: bytes,
        totalBytes: bytes,
      });
      return {
        operationId: params.operationId,
        ref,
        bytes,
        staged: ref !== params.localPath,
      };
    },
    async adopt() {},
    async cancel(operationId) {
      getEmitter(operationId).fire({
        operationId,
        phase: "canceled",
        uploadedBytes: 0,
        totalBytes: 0,
      });
    },
    async cleanup() {},
    onDynamicProgress: (operationId) => getEmitter(operationId).event,
  };
}
