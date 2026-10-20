/**
 * 多选文件能力只在壳真的实现时才暴露给 `IPlatformService`。
 *
 * 反例（2026-10-20 真机）：`selectFiles` 被定义成「返回空数组」的兜底后，
 * `openAttachmentPicker` 看到 `platform.selectFiles` 存在就直接调用，拿到 `[]` 便结束，
 * 不再回退到单选 `selectFile`——Tauri 壳没有实现多选命令，于是 "+" 按钮静默无响应。
 */
export function resolveSelectFilesCapability(selectFiles: (() => Promise<string[]>) | undefined): {
  selectFiles?: () => Promise<string[]>;
} {
  return selectFiles ? { selectFiles: () => selectFiles() } : {};
}
