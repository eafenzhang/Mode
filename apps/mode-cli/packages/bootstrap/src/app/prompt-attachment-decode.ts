// 宿主侧附件 data URL 解码：`readSessionAttachment` → `readPromptAttachment` 唯一经过的
// artifact 字节解码路径，供两条读路径共用——`attachmentRead`（会话内预览/下载）与
// `conversationAttachmentRead`（share 读面）。
//
// 这里曾按 image/video/pdf 抛 `fault.attachment.previewNotMedia`，与 gateway 侧判定形成
// 双重卡死：两条读路径在 gateway 都已按权威行授权（`authorizeConversationAttachment`
// mediaOnly:false，MIME 从来不是授权因子；share 侧暴露面由发布时的 capabilities
// mime/扩展名白名单决定，也不在读路径上），host 再按 MIME 拦一次只会把 txt/md/office
// 的合法下载全部打死，renderer 还把 `previewNotMedia` 误判成「对端版本过低」
// （依据：docs/specs/remote-workspace-bidirectional-attachments.md 收向链路
// 「权威行 ref 精确匹配（任意 MIME）」与验收场景 1/7/10）。
//
// 因此本函数只保留结构与容量语义：
// - 头部/base64 结构非法 → `fault.attachment.previewArtifactInvalid`；
// - 解码后字节 > maxBytes → `fault.attachment.previewTooLarge`
//   （maxBytes 由 gateway 按 PROTOCOL_V4_LIMITS 下发：非视频 20 MiB、视频 30 MiB）。
//
// 媒体 gate 不在这里：`attachmentPreviewSource` 的媒体限制由 attachment-row-auth 的
// mediaOnly:true 独立承担，改动互不影响。
export function decodePromptAttachmentDataUrl(
  content: string,
  fallbackMime: string,
  maxBytes: number,
): { bytes: Uint8Array; mediaType: string } {
  const commaIndex = content.indexOf(",");
  const headerParts =
    content.slice(0, "data:".length).toLowerCase() === "data:" && commaIndex >= 0
      ? content.slice("data:".length, commaIndex).split(";")
      : [];
  const mediaType = (headerParts.shift()?.trim() || fallbackMime).split(";", 1)[0]!.toLowerCase();
  const payload = commaIndex >= 0 ? content.slice(commaIndex + 1) : "";
  if (
    headerParts.at(-1)?.trim().toLowerCase() !== "base64" ||
    !/^[A-Za-z0-9+/]*={0,2}$/u.test(payload) ||
    payload.length % 4 !== 0
  ) {
    throw new Error("fault.attachment.previewArtifactInvalid");
  }
  const bytes = Buffer.from(payload, "base64");
  if (bytes.byteLength > maxBytes) {
    throw new Error("fault.attachment.previewTooLarge");
  }
  return { bytes, mediaType };
}
