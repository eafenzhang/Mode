const OPEN_ACCOUNT_DIALOG_EVENT = "mode:open-account-dialog";

/**
 * 账号弹窗目前挂在侧边栏页脚，只有侧栏菜单能打开。远程控制页的「登录账号」空态
 * 需要一个不依赖侧栏状态的入口：用事件通知宿主打开，避免把弹窗状态提升成全局面板。
 */
export function openModeAccountDialog(): void {
  if (typeof window === "undefined") return;
  window.dispatchEvent(new Event(OPEN_ACCOUNT_DIALOG_EVENT));
}

export function addModeAccountDialogOpenListener(listener: () => void): () => void {
  if (typeof window === "undefined") return () => {};
  window.addEventListener(OPEN_ACCOUNT_DIALOG_EVENT, listener);
  return () => window.removeEventListener(OPEN_ACCOUNT_DIALOG_EVENT, listener);
}
