/**
 * 「帮助 → 检查更新」的结果反馈。
 *
 * Rust 侧 `check_and_install` 已经返回 `{status}`（up-to-date/error/busy），但渲染层此前
 * 忽略返回值，用户点完看不到任何变化（2026-10-19 本机验收：检查更新没反应）。
 * 这里只做纯映射：结果 → 文案，由调用方负责 toast。
 */
export interface UpdateCheckFeedback {
  message: string;
  variant: "info" | "warning";
}

type FormatMessage = (id: string, values?: Record<string, string | number>) => string;

function asRecord(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

export const UPDATE_CHECK_TOAST_DEDUPE_KEY = "desktop-update-check";

export interface RunUpdateCheckOptions {
  execute: () => Promise<unknown>;
  formatMessage: FormatMessage;
  showToast: (message: string, options: UpdateCheckToastOptions) => void;
}

export interface UpdateCheckToastOptions {
  variant: UpdateCheckFeedback["variant"];
  dedupeKey: string;
}

/**
 * 执行一次手动检查并把结果转成可见 toast。
 *
 * 单独抽出来是因为「检查更新没反应」的根因不是映射函数，而是调用点只 import 没使用：
 * 把「执行 → 映射 → 提示」固化成一条可测路径，避免再次出现只有纯函数单测、点击却静默。
 */
export async function runUpdateCheck(options: RunUpdateCheckOptions): Promise<void> {
  try {
    const feedback = updateCheckFeedback(await options.execute(), options.formatMessage);
    if (feedback) {
      options.showToast(feedback.message, {
        variant: feedback.variant,
        dedupeKey: UPDATE_CHECK_TOAST_DEDUPE_KEY,
      });
    }
  } catch (error) {
    // Tauri 侧检查更新要联网；invoke 拒绝时也必须给出可见反馈。
    options.showToast(
      options.formatMessage("update.toast.error", {
        error: error instanceof Error ? error.message : String(error),
      }),
      { variant: "warning", dedupeKey: UPDATE_CHECK_TOAST_DEDUPE_KEY },
    );
  }
}

export function updateCheckFeedback(
  result: unknown,
  formatMessage: FormatMessage,
): UpdateCheckFeedback | null {
  const record = asRecord(result);
  if (!record) return null;
  const status = record["status"];
  if (status === "up-to-date") {
    return {
      message: formatMessage("update.toast.upToDate", {
        version: typeof record["currentVersion"] === "string" ? record["currentVersion"] : "?",
      }),
      variant: "info",
    };
  }
  if (status === "busy") {
    return { message: formatMessage("desktopMenu.help.checkingForUpdates"), variant: "info" };
  }
  if (status === "error") {
    return {
      message: formatMessage("update.toast.error", {
        error: typeof record["error"] === "string" ? record["error"] : "unknown",
      }),
      variant: "warning",
    };
  }
  return null;
}
