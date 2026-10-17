// Mode browser-import extension popup.
//
// Per spec `docs/specs/chrome-browser-import-extension.md`:
// - user action only (no background reads)
// - reads cookies via the supported `chrome.cookies` API
// - sends the minimal fields to the native host
// - never uploads anything to the network
const HOST_NAME = "com.mode.browser_import";
const MAX_MESSAGE_BYTES = 1024 * 1024;

type NativeResponse = { ok?: boolean; count?: number; error?: string };

const button = document.getElementById("import") as HTMLButtonElement;
const statusText = document.getElementById("status") as HTMLParagraphElement;

let finished = false;

// popup 在导入完成前被关闭：尽力通知 native host，让 Mode 立即结束等待。
// 页面卸载时消息可能被中断，因此主流程仍以自身超时兜底。
window.addEventListener("pagehide", () => {
  if (finished) return;
  void chrome.runtime
    .sendNativeMessage(HOST_NAME, { v: 1, kind: "cancel" })
    .catch(() => undefined);
});

button.addEventListener("click", () => {
  void runImport();
});

async function runImport(): Promise<void> {
  finished = false;
  button.disabled = true;
  statusText.textContent = "读取 cookie…";
  try {
    const cookies = await chrome.cookies.getAll({});
    const payload = {
      v: 1,
      kind: "cookies",
      cookies: cookies.map((cookie) => ({
        name: cookie.name,
        value: cookie.value,
        domain: cookie.domain,
        path: cookie.path,
        secure: cookie.secure,
        httpOnly: cookie.httpOnly,
        sameSite: cookie.sameSite,
        expirationDate: cookie.expirationDate,
        hostOnly: cookie.hostOnly,
      })),
    };
    const bytes = new TextEncoder().encode(JSON.stringify(payload)).byteLength;
    if (bytes > MAX_MESSAGE_BYTES) {
      statusText.textContent = `失败：payload_too_large（${bytes} 字节）`;
      return;
    }
    statusText.textContent = `发送 ${cookies.length} 条 cookie…`;
    const raw = await chrome.runtime.sendNativeMessage(HOST_NAME, payload);
    const response = raw as NativeResponse | null;
    if (response?.ok === true) {
      statusText.textContent = `已导入 ${response.count ?? 0} 条`;
      return;
    }
    statusText.textContent = `失败：${response?.error ?? "native_host_missing"}`;
  } catch (error) {
    statusText.textContent = `失败：${error instanceof Error ? error.message : String(error)}`;
  } finally {
    finished = true;
    button.disabled = false;
  }
}
