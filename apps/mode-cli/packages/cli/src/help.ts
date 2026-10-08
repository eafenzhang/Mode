import { getModeCopy, type SupportedLocale, type UiLocale } from "@mode/i18n";

export function formatCliHelp(
  version: string,
  locale?: UiLocale,
  detectedLocale?: SupportedLocale,
): string {
  return getModeCopy(locale, detectedLocale).cli.help(version);
}
