// 会话历史附件下载编排：读字节（经权威行授权的 attachmentRead）→ 平台另存/浏览器回退。
// 不请求 mediaType——transport 的 local_path 优化只对 mediaType 生效，下载必须拿完整字节。
import type { AttachmentRef, ConversationRowTarget } from "@mode/shared/mode-protocol-v4";

export type AttachmentDownloadFailure =
  | "tooLarge"
  | "unauthorized"
  | "legacyPeer"
  | "unsupported"
  | "unknown";

export type AttachmentReadResult =
  | { bytes: Uint8Array; mediaType: string }
  | { url: string; mediaType: string };

export interface AttachmentDownloadReadParams {
  sessionId: string;
  ref: string;
  signal?: AbortSignal;
  target?: ConversationRowTarget;
  attachmentIndex?: number;
}

export interface SaveFilePayload {
  data: ArrayBuffer;
  suggestedName: string;
}
export type SaveFileResult = {
  canceled: boolean;
  success: boolean;
  path?: string;
  error?: string;
};
export type SaveFileFn = (payload: SaveFilePayload) => Promise<SaveFileResult>;

export interface DownloadAttachmentDeps {
  readAttachment: (params: AttachmentDownloadReadParams) => Promise<AttachmentReadResult>;
  saveFile?: SaveFileFn;
  triggerBrowserDownload?: (bytes: Uint8Array, mediaType: string, fileName: string) => void;
}

export type AttachmentDownloadOutcome =
  | { status: "saved"; path: string }
  | { status: "browserDownloaded" }
  | { status: "canceled" }
  | { status: "failed"; failure: AttachmentDownloadFailure; message: string };

export interface DownloadAttachmentParams {
  sessionId: string;
  ref: string;
  fileName: string;
  target?: ConversationRowTarget;
  attachmentIndex?: number;
  signal?: AbortSignal;
}

export function classifyAttachmentDownloadError(error: unknown): AttachmentDownloadFailure {
  const message = error instanceof Error ? error.message : String(error);
  if (message.includes("previewTooLarge")) return "tooLarge";
  if (message.includes("previewRefNotAuthorized")) return "unauthorized";
  // 旧版本对端网关仍卡媒体：previewNotMedia 是兼容分类，不是本端缺陷。
  if (message.includes("previewNotMedia")) return "legacyPeer";
  if (message.includes("readUnsupported")) return "unsupported";
  return "unknown";
}

export function suggestedAttachmentDownloadName(fileName: string): string {
  const withoutSeparators = fileName.replace(/[/\\]/gu, "");
  // 剔除控制字符（码点小于空格或 DEL）并去首尾空白；用码点比较而非正则字符类，
  // 避免源码里携带原始控制字节。
  const cleaned = [...withoutSeparators]
    .filter((ch) => ch >= " " && ch.codePointAt(0) !== 0x7f)
    .join("")
    .trim();
  return cleaned.length > 0 ? cleaned : "attachment";
}

export interface BuildAttachmentDownloadReadParamsInput {
  attachment: Pick<AttachmentRef, "ref" | "fileName" | "mime" | "previewRef">;
  sessionId: string;
  rowId: number;
  entityId?: string;
  attachmentIndices?: readonly number[];
  index: number;
  /** artifact 行没有附件序号，传 false 时只带 target（授权走 target 无 index 分支）。 */
  includeAttachmentIndex?: boolean;
}

export function buildAttachmentDownloadReadParams(
  input: BuildAttachmentDownloadReadParamsInput,
): AttachmentDownloadReadParams {
  const includeIndex = input.includeAttachmentIndex ?? true;
  return {
    sessionId: input.sessionId,
    ref: input.attachment.previewRef ?? input.attachment.ref,
    ...(input.entityId ? { target: { rowId: input.rowId, entityId: input.entityId } } : {}),
    ...(input.entityId && includeIndex
      ? { attachmentIndex: input.attachmentIndices?.[input.index] ?? input.index }
      : {}),
  };
}

export async function downloadAttachment(
  deps: DownloadAttachmentDeps,
  params: DownloadAttachmentParams,
): Promise<AttachmentDownloadOutcome> {
  try {
    const result = await deps.readAttachment({
      sessionId: params.sessionId,
      ref: params.ref,
      ...(params.target ? { target: params.target } : {}),
      ...(params.attachmentIndex !== undefined ? { attachmentIndex: params.attachmentIndex } : {}),
      ...(params.signal ? { signal: params.signal } : {}),
    });
    if ("url" in result) {
      // 不带 mediaType 的读取不应命中 local_path 分支；防御性失败，不伪造下载成功。
      return {
        status: "failed",
        failure: "unsupported",
        message: "attachment read returned local url",
      };
    }
    const suggestedName = suggestedAttachmentDownloadName(params.fileName);
    if (deps.saveFile) {
      const saved = await deps.saveFile({
        data: bytesToArrayBuffer(result.bytes),
        suggestedName,
      });
      if (saved.canceled) return { status: "canceled" };
      if (!saved.success || !saved.path) {
        return { status: "failed", failure: "unknown", message: saved.error ?? "save_failed" };
      }
      return { status: "saved", path: saved.path };
    }
    if (!deps.triggerBrowserDownload) {
      return { status: "failed", failure: "unsupported", message: "no download sink" };
    }
    deps.triggerBrowserDownload(result.bytes, result.mediaType, suggestedName);
    return { status: "browserDownloaded" };
  } catch (error) {
    return {
      status: "failed",
      failure: classifyAttachmentDownloadError(error),
      message: error instanceof Error ? error.message : String(error),
    };
  }
}

function bytesToArrayBuffer(bytes: Uint8Array): ArrayBuffer {
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
}
