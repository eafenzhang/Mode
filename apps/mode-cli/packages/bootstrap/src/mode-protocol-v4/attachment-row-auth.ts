// 附件读授权（v4-gateway 附件读面唯一裁决点）。
// 规则：ref 必须命中当前 session 权威投影中的行——userInput 行按 ref/previewRef
// 精确匹配，artifact 行按 ref 精确匹配，assistant markdown 只认本 session 的
// mode-artifact URI；跨 session、任意路径一律 null。mediaOnly 只是媒体 gate
// （attachmentPreviewSource 旧语义），不参与授权本身。
import { extractMarkdownArtifactImageRefs } from "@mode/shared";
import type { AttachmentRef, ConversationRow } from "@mode/shared/mode-protocol-v4";

export interface ConversationRowAuthTarget {
  rowId: number;
  entityId: string;
}

export interface AuthorizedConversationAttachment {
  attachment: AttachmentRef;
  /** 仅 userInput target+index 授权携带（与旧实现的 messageId 语义一致）。 */
  attachmentIndex?: number;
  sourceRowId?: number;
}

export interface AuthorizeConversationAttachmentParams {
  sessionId: string;
  ref: string;
  target?: ConversationRowAuthTarget;
  attachmentIndex?: number;
  /** true = 只放行 image/video/pdf（previewSource 语义）；false = 任意 MIME（read 语义）。 */
  mediaOnly: boolean;
}

/** 仅本模块内用：ref 的 URI authority 必须就是当前请求的 session（依据见 assistantText 分支）。 */
function artifactRefBelongsToSession(ref: string, sessionId: string): boolean {
  return ref.startsWith(`mode-artifact://${encodeURIComponent(sessionId)}/`);
}

function isPreviewableMime(mime: string): boolean {
  const normalized = mime.split(";", 1)[0]?.trim().toLowerCase() ?? "";
  return (
    normalized.startsWith("image/") ||
    normalized.startsWith("video/") ||
    normalized === "application/pdf"
  );
}

function matchesRef(attachment: AttachmentRef, ref: string): boolean {
  return attachment.ref === ref || attachment.previewRef === ref;
}

function artifactRowAttachmentRef(
  row: Extract<ConversationRow, { kind: "artifact" }>,
): AttachmentRef {
  return {
    ref: row.ref,
    fileName: row.displayName,
    mime: row.mimeType,
    bytes: row.sizeBytes,
  };
}

export function authorizeConversationAttachment(
  rows: readonly ConversationRow[],
  params: AuthorizeConversationAttachmentParams,
): AuthorizedConversationAttachment | null {
  const { sessionId, ref, target, attachmentIndex, mediaOnly } = params;
  const allowed = (mime: string) => !mediaOnly || isPreviewableMime(mime);

  if (target && attachmentIndex !== undefined) {
    const row = rows.find(
      (candidate) => candidate.rowId === target.rowId && candidate.entityId === target.entityId,
    );
    if (row?.kind !== "userInput") return null;
    const attachment = row.attachments?.[attachmentIndex];
    if (!attachment || !matchesRef(attachment, ref) || !allowed(attachment.mime)) return null;
    // 热态 renderer 可能还持有 original ref，而 hydrate 后的权威 row 已补
    // previewRef；两者属于同一个 row/index，授权不能因投影时序不同而误判为跨行读取。
    return { attachment, attachmentIndex, sourceRowId: row.rowId };
  }

  if (target) {
    // 带 target 无 index：只按行身份授权 artifact（ref 精确匹配）；不误授权该行其他附件。
    const row = rows.find(
      (candidate) => candidate.rowId === target.rowId && candidate.entityId === target.entityId,
    );
    if (row?.kind === "artifact" && row.ref === ref && allowed(row.mimeType)) {
      return { attachment: artifactRowAttachmentRef(row) };
    }
    if (row?.kind === "artifact") return null;
    // userInput 行缺 index 无法定位附件，退回扫描（与旧实现「target 未命中即全量扫描」对齐）。
  }

  for (const row of rows) {
    if (row.kind === "userInput") {
      for (const attachment of row.attachments ?? []) {
        if (!allowed(attachment.mime)) continue;
        if ((attachment.previewRef ?? attachment.ref) === ref) {
          // 旧扫描分支不携带 index/messageId（缓存 key 走 legacy），保持不变。
          return { attachment };
        }
      }
      continue;
    }
    if (row.kind === "artifact" && row.ref === ref && allowed(row.mimeType)) {
      return { attachment: artifactRowAttachmentRef(row) };
    }
    if (
      row.kind === "assistantText" &&
      artifactRefBelongsToSession(ref, sessionId) &&
      extractMarkdownArtifactImageRefs(row.text).includes(ref)
    ) {
      // assistant Markdown 可以引用工具产出的 session artifact，但仅看 userInput.attachments
      // 会让合法引用在 UI 里被 harden 拦截；因此仍以当前 session 的权威投影做精确 ref 授权，
      // 绝不接受 renderer 自报的任意 artifact/path。Markdown 是模型可控文本，所以 URI
      // authority 还必须与当前请求 session 精确匹配；仅「当前投影里出现过」不能证明它有权
      // 读取另一个 session 的 artifact。
      return { attachment: { ref, fileName: "assistant-image", mime: "image/*", bytes: 0 } };
    }
  }
  return null;
}
