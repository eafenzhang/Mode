// LAN 远端工作区附件传输：对端是已运行的 Mode 实例、无 stdio backend，
// 没有 backend.upload 可用；改走会话 artifact 的 begin/chunk/commit 字节通道
// （与手机远控同一条已验证链路），对 renderer 仍暴露同一个 stage 契约。
// 单文件上限 = PROTOCOL_V4_LIMITS.attachmentMaxBytes（20 MiB，spec 已确认不调整）。
import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { stat } from "node:fs/promises";
import { Emitter } from "@mode/rpc";
import type { ModePromptAttachment, TraceId } from "@mode/shared";
import { PROTOCOL_V4_LIMITS } from "@mode/shared/mode-protocol-v4";
import type {
  IFileService,
  IPromptAttachmentTransferService,
  IModeAgentService,
  PromptAttachmentStageParams,
  PromptAttachmentStageResult,
  PromptAttachmentTransferProgress,
} from "@mode/services";
import {
  REMOTE_PROMPT_ATTACHMENT_ROOT,
  isPathInRoot,
  replaceContentPaths,
} from "./remotePromptAttachments.js";

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

export interface LanMaterializeInput {
  taskId: string;
  traceId: TraceId | string;
  content: string;
  attachments?: ModePromptAttachment[];
}

export interface LanMaterializeOptions {
  fileService: Pick<IFileService, "readdir">;
  /** 测试注入；缺省 = fs stat + 流式读 → base64（≤ attachmentMaxBytes）。 */
  readFileBase64?: (path: string) => Promise<{ dataBase64: string; bytes: number }>;
}

const LAN_INLINE_LIMIT_MARKER = "局域网附件单文件上限 20 MiB";

async function defaultReadFileBase64(path: string): Promise<{ dataBase64: string; bytes: number }> {
  const info = await stat(path);
  if (!info.isFile()) {
    throw Object.assign(new Error(`附件不是文件：${path}`), { code: "LAN_INLINE_NOT_FILE" });
  }
  if (info.size > PROTOCOL_V4_LIMITS.attachmentMaxBytes) {
    const base = path.split(/[/\\]/u).at(-1) ?? path;
    throw Object.assign(new Error(`${LAN_INLINE_LIMIT_MARKER}：${base}`), {
      code: "LAN_INLINE_TOO_LARGE",
    });
  }
  const parts: Buffer[] = [];
  for await (const piece of createReadStream(path)) parts.push(piece as Buffer);
  return { dataBase64: Buffer.concat(parts).toString("base64"), bytes: info.size };
}

function isNotFoundLike(error: unknown): boolean {
  if (!(error instanceof Error)) return false;
  return error.message.includes("ENOENT") || (error as { code?: string }).code === "ENOENT";
}

function isTooLargeLike(error: unknown): boolean {
  if (!(error instanceof Error)) return false;
  return (
    error.message.includes(LAN_INLINE_LIMIT_MARKER) ||
    (error as { code?: string }).code === "LAN_INLINE_TOO_LARGE"
  );
}

function dirnameLikePath(path: string): string {
  const index = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  if (index < 0) return ".";
  if (index === 0) return path.slice(0, 1);
  return path.slice(0, index);
}

function basenameLikePath(path: string): string {
  const index = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return index >= 0 ? path.slice(index + 1) : path;
}

async function remoteFileExists(
  fileService: Pick<IFileService, "readdir">,
  path: string,
): Promise<boolean> {
  try {
    const entries = await fileService.readdir({ path: dirnameLikePath(path), includeHidden: true });
    const base = basenameLikePath(path);
    return entries.some(
      (entry) => entry.path === path || (entry.name === base && entry.type === "file"),
    );
  } catch {
    return false;
  }
}

export async function materializeLanPromptAttachments(
  input: LanMaterializeInput,
  options: LanMaterializeOptions,
): Promise<{ content: string; attachments?: ModePromptAttachment[]; uploadedCount: number }> {
  const attachments = input.attachments;
  if (!attachments || attachments.length === 0) {
    return { content: input.content, attachments: input.attachments, uploadedCount: 0 };
  }
  const replacements = new Map<string, string>();
  const nextAttachments: ModePromptAttachment[] = [];
  let uploadedCount = 0;
  let changed = false;

  for (const attachment of attachments) {
    const localPath = attachment.localPath?.trim();
    if (!localPath) {
      nextAttachments.push(attachment);
      continue;
    }
    if (isPathInRoot(localPath, REMOTE_PROMPT_ATTACHMENT_ROOT)) {
      // 与 SSH 对齐：staging 根路径已属于对端，透传。
      nextAttachments.push(attachment);
      continue;
    }
    try {
      const inline = await (options.readFileBase64 ?? defaultReadFileBase64)(localPath);
      const { localPath: _dropped, ...rest } = attachment;
      nextAttachments.push({ ...rest, dataBase64: inline.dataBase64 } as ModePromptAttachment);
      // 内容里引用本机路径的位置改写为文件名：主机路径永不抵达对端（不变量 1）。
      replacements.set(localPath, attachment.filename);
      uploadedCount += 1;
      changed = true;
    } catch (error) {
      if (isTooLargeLike(error)) throw error;
      if (!isNotFoundLike(error)) {
        throw new Error(`本地附件读取失败：${attachment.filename}`, { cause: error });
      }
      if (await remoteFileExists(options.fileService, localPath)) {
        // 遗留转换把对端路径落成 localPath：对端本就可读，原样放行。
        nextAttachments.push(attachment);
        continue;
      }
      throw new Error(`附件路径在本机和远端都不存在：${localPath}`);
    }
  }

  return {
    content: replaceContentPaths(input.content, replacements),
    attachments: changed ? nextAttachments : attachments,
    uploadedCount,
  };
}
