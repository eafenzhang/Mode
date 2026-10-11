// LAN 远端工作区附件传输：对端是已运行的 Mode 实例、无 stdio backend，
// 没有 backend.upload 可用；改走会话 artifact 的 begin/chunk/commit 字节通道
// （与手机远控同一条已验证链路），对 renderer 仍暴露同一个 stage 契约。
// 单文件上限 = PROTOCOL_V4_LIMITS.attachmentMaxBytes（20 MiB，spec 已确认不调整）。
import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { stat } from "node:fs/promises";
import { Emitter } from "@mode/rpc";
import { PROTOCOL_V4_LIMITS } from "@mode/shared/mode-protocol-v4";
import type {
  IPromptAttachmentTransferService,
  IModeAgentService,
  PromptAttachmentStageParams,
  PromptAttachmentStageResult,
  PromptAttachmentTransferProgress,
} from "@mode/services";

export type LanAttachmentChannel = Pick<
  IModeAgentService,
  "attachmentBeginV4" | "attachmentChunkV4" | "attachmentCommitV4" | "attachmentAbortV4"
>;

const CHUNK_BYTES = PROTOCOL_V4_LIMITS.attachmentChunkMaxBytes;
const LAN_STAGE_LIMIT_MESSAGE =
  "局域网连接附件单文件上限 20 MiB，请改用 SSH/WSL/Docker 工作区，或先把文件放到对端工作区。";
const LAN_PUT_UNSUPPORTED_MESSAGE = "对端版本不支持附件上传";

interface LanStageTarget {
  workspacePath: string;
  workspaceIdentity?: string;
  sessionId: string;
}

interface LanStageRecord {
  controller: AbortController;
  target: LanStageTarget;
  committed: boolean;
}

/** uploadId 必须匹配 ^[A-Za-z0-9][A-Za-z0-9._:-]*$（≤128）；operationId 来自 nanoid，先归一。 */
function toUploadId(operationId: string): string {
  const cleaned = operationId.replace(/[^A-Za-z0-9._:-]+/gu, "-");
  const started = /^[A-Za-z0-9]/u.test(cleaned) ? cleaned : `u-${cleaned}`;
  return started.slice(0, 128);
}

async function sha256OfFile(path: string): Promise<string> {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(path)) {
    hash.update(chunk as Buffer);
  }
  return hash.digest("hex");
}

async function readRange(path: string, start: number, end: number): Promise<Buffer> {
  const parts: Buffer[] = [];
  for await (const piece of createReadStream(path, { start, end })) {
    parts.push(piece as Buffer);
  }
  return Buffer.concat(parts);
}

function abortError(): Error {
  return Object.assign(new Error("LAN attachment stage canceled"), { name: "AbortError" });
}

function isAbortLike(error: unknown): boolean {
  return error instanceof Error && error.name === "AbortError";
}

export function createLanPromptAttachmentTransferService(deps: {
  channel: LanAttachmentChannel;
}): IPromptAttachmentTransferService {
  const records = new Map<string, LanStageRecord>();
  const emitters = new Map<string, Emitter<PromptAttachmentTransferProgress>>();

  const getEmitter = (operationId: string) => {
    const existing = emitters.get(operationId);
    if (existing) return existing;
    const emitter = new Emitter<PromptAttachmentTransferProgress>({
      onDidRemoveLastListener: () => {
        emitters.delete(operationId);
        emitter.dispose();
      },
    });
    emitters.set(operationId, emitter);
    return emitter;
  };

  const abortRemote = async (target: LanStageTarget, uploadId: string) => {
    await deps.channel.attachmentAbortV4({ ...target, uploadId }).catch(() => {});
  };

  const cancelInternal = async (operationId: string) => {
    const record = records.get(operationId);
    if (!record) return;
    record.controller.abort();
    records.delete(operationId);
    await abortRemote(record.target, toUploadId(operationId));
    getEmitter(operationId).fire({
      operationId,
      phase: "canceled",
      uploadedBytes: 0,
      totalBytes: 0,
    });
  };

  return {
    async stage(params: PromptAttachmentStageParams): Promise<PromptAttachmentStageResult> {
      await cancelInternal(params.operationId);
      const target: LanStageTarget = {
        workspacePath: params.workspacePath,
        ...(params.workspaceIdentity ? { workspaceIdentity: params.workspaceIdentity } : {}),
        sessionId: params.sessionId,
      };
      const record: LanStageRecord = {
        controller: new AbortController(),
        target,
        committed: false,
      };
      records.set(params.operationId, record);
      const emitter = getEmitter(params.operationId);
      const uploadId = toUploadId(params.operationId);
      try {
        const info = await stat(params.localPath).catch((error: unknown) => {
          const message = error instanceof Error ? error.message : String(error);
          if (message.includes("ENOENT")) {
            throw new Error(`附件文件不存在：${params.localPath}`);
          }
          throw error;
        });
        if (!info.isFile()) throw new Error(`附件不是文件：${params.fileName}`);
        if (info.size > PROTOCOL_V4_LIMITS.attachmentMaxBytes) {
          throw new Error(LAN_STAGE_LIMIT_MESSAGE);
        }
        const totalBytes = info.size;
        const checksum = `sha256:${await sha256OfFile(params.localPath)}`;
        const totalChunks = totalBytes === 0 ? 0 : Math.ceil(totalBytes / CHUNK_BYTES);
        const begin = await deps.channel.attachmentBeginV4({
          ...target,
          uploadId,
          fileName: params.fileName,
          mime: params.mime,
          totalBytes,
          totalChunks,
          checksum,
        });

        let ref: string;
        if (begin.state === "committed") {
          // 幂等续传：begin ACK 丢失后重试，对端已提交则直接采用既有 ref。
          ref = begin.ref;
        } else {
          let uploadedBytes = 0;
          emitter.fire({
            operationId: params.operationId,
            phase: "uploading",
            uploadedBytes,
            totalBytes,
          });
          for (let chunkIndex = begin.nextChunkIndex; chunkIndex < totalChunks; chunkIndex += 1) {
            if (record.controller.signal.aborted || records.get(params.operationId) !== record) {
              throw abortError();
            }
            const start = chunkIndex * CHUNK_BYTES;
            const end = Math.min(start + CHUNK_BYTES, totalBytes) - 1;
            const dataBase64 = (await readRange(params.localPath, start, end)).toString("base64");
            if (record.controller.signal.aborted || records.get(params.operationId) !== record) {
              throw abortError();
            }
            await deps.channel.attachmentChunkV4({
              ...target,
              uploadId,
              chunkIndex,
              dataBase64,
            });
            uploadedBytes = end + 1;
            emitter.fire({
              operationId: params.operationId,
              phase: "uploading",
              uploadedBytes,
              totalBytes,
            });
          }
          emitter.fire({
            operationId: params.operationId,
            phase: "committing",
            uploadedBytes: totalBytes,
            totalBytes,
          });
          const commit = await deps.channel.attachmentCommitV4({ ...target, uploadId });
          ref = commit.ref;
        }
        record.committed = true;
        records.delete(params.operationId);
        emitter.fire({
          operationId: params.operationId,
          phase: "complete",
          uploadedBytes: totalBytes,
          totalBytes,
        });
        return { operationId: params.operationId, ref, bytes: totalBytes, staged: true };
      } catch (error) {
        if (!record.committed) {
          await abortRemote(target, uploadId);
        }
        if (records.get(params.operationId) === record) {
          records.delete(params.operationId);
        }
        if (isAbortLike(error) || record.controller.signal.aborted) {
          emitter.fire({
            operationId: params.operationId,
            phase: "canceled",
            uploadedBytes: 0,
            totalBytes: 0,
          });
          throw error;
        }
        const message = error instanceof Error ? error.message : String(error);
        if (message === "fault.attachment.putUnsupported") {
          throw new Error(LAN_PUT_UNSUPPORTED_MESSAGE, { cause: error });
        }
        // 本地确定性错误（超限/不是文件/不存在）原样上抛，保住可读文案与永久失败分类。
        if (
          message === LAN_STAGE_LIMIT_MESSAGE ||
          message.startsWith("附件不是文件") ||
          message.startsWith("附件文件不存在")
        ) {
          throw error;
        }
        throw new Error(`局域网附件上传失败：${params.fileName}`, { cause: error });
      }
    },
    async adopt() {
      // artifact store 对未引用条目按 attachmentUnreferencedTtlMs(24h) 自动回收；
      // 已提交的 LAN 附件无远端路径需要保留或删除。
    },
    async cancel(operationId) {
      await cancelInternal(operationId);
    },
    async cleanup(operationId) {
      await cancelInternal(operationId);
    },
    onDynamicProgress: (operationId) => getEmitter(operationId).event,
  };
}
