import type { ChromeBrowserDataImportError, ChromeBrowserDataImportResult } from "@mode/shared";
import type { useModeIntl } from "@/i18n/IntlProvider.js";

/**
 * 每个稳定错误码对应一条可操作提示。扩展通道的码见
 * `docs/specs/chrome-browser-import-extension.md` §5；未列出的码统一落到 failed。
 */
const IMPORT_ERROR_MESSAGE_IDS: Partial<Record<ChromeBrowserDataImportError, string>> = {
  chrome_profile_not_found: "settings.browser.import.notFound",
  chrome_default_profile_not_found: "settings.browser.import.notFound",
  chrome_profile_ambiguous: "settings.browser.import.ambiguous",
  chrome_executable_not_found: "settings.browser.import.executableNotFound",
  chrome_cookie_access_denied: "settings.browser.import.accessDenied",
  chrome_cookie_elevation_required: "settings.browser.import.elevationRequired",
  chrome_cookie_elevation_cancelled: "settings.browser.import.elevationCancelled",
  chrome_cookie_helper_verification_failed: "settings.browser.import.helperVerificationFailed",
  chrome_cookie_app_bound_decryption_failed: "settings.browser.import.appBoundFailed",
  chrome_cookie_protection_unsupported: "settings.browser.import.cookieProtected",
  chrome_profile_locked: "settings.browser.import.profileLocked",
  chrome_local_storage_import_failed: "settings.browser.import.localStorageFailed",
  channel_missing: "settings.browser.import.extensionChannelFailed",
  channel_timeout: "settings.browser.import.extensionTimeout",
  channel_token_mismatch: "settings.browser.import.extensionChannelFailed",
  payload_too_large: "settings.browser.import.extensionPayloadTooLarge",
  invalid_payload: "settings.browser.import.extensionInvalidPayload",
  cookie_write_failed: "settings.browser.import.extensionWriteFailed",
  import_cancelled: "settings.browser.import.extensionCancelled",
  extension_missing: "settings.browser.import.extensionMissing",
  native_host_missing: "settings.browser.import.nativeHostMissing",
};

export function formatImportSummary(
  result: ChromeBrowserDataImportResult,
  formatMessage: ReturnType<typeof useModeIntl>["intl"]["formatMessage"],
  source: "native" | "extension" = "native",
): string {
  if (!result.success) {
    const errorMessageId =
      (result.error ? IMPORT_ERROR_MESSAGE_IDS[result.error] : undefined) ??
      "settings.browser.import.failed";
    return formatMessage({ id: errorMessageId });
  }
  if (source === "extension") {
    // 扩展通道只导入 Cookie，没有 LocalStorage；避免复用会显示 0 条 LocalStorage 的文案。
    return formatMessage(
      { id: "settings.browser.import.extensionSuccess" },
      { cookies: String(result.cookies.imported) },
    );
  }
  const values = {
    cookies: String(result.cookies.imported),
    origins: String(result.localStorage.originsImported),
    entries: String(result.localStorage.entriesImported),
    skipped: String(result.cookies.skipped),
  };
  if (
    result.issues?.some((issue) =>
      [
        "chrome_cookie_elevation_required",
        "chrome_cookie_elevation_cancelled",
        "chrome_cookie_helper_verification_failed",
        "chrome_cookie_app_bound_decryption_failed",
      ].includes(issue),
    )
  ) {
    return formatMessage({ id: "settings.browser.import.partialAppBound" }, values);
  }
  // Rebase 后的产品语义要求成功态只展示实际导入数量；普通 skipped/failed 详情留在安全日志。
  // App-Bound 授权/校验失败是用户刚刚显式确认的动作，单独保留部分成功提示，避免误以为 Cookie 已导入。
  return formatMessage({ id: "settings.browser.import.success" }, values);
}
