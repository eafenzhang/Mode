import type { UiLocale, SupportedLocale } from "@mode/contracts";
import { enUS } from "./locales/en-US.js";
import { zhCN } from "./locales/zh-CN.js";
import { faIR } from "./locales/fa.js";
import {
  DEFAULT_LOCALE,
  detectLocale,
  isSupportedLocale,
  isUiLocale,
  resolveLocale,
  SUPPORTED_LOCALES,
} from "./locale.js";
import type { ModeCopy } from "./types.js";

export {
  DEFAULT_LOCALE,
  SUPPORTED_LOCALES,
  detectLocale,
  isSupportedLocale,
  isUiLocale,
  resolveLocale,
};
export type { LocaleDetectionInput } from "./locale.js";
export type { CliCopy, TuiCopy, UiLocale, SupportedLocale, ModeCopy } from "./types.js";

const CATALOGS: Record<SupportedLocale, ModeCopy> = {
  "en-US": enUS,
  "zh-CN": zhCN,
  "fa-IR": faIR,
};

export function getModeCopy(locale?: UiLocale | string, detected?: string | null): ModeCopy {
  return CATALOGS[resolveLocale(locale, detected)];
}
