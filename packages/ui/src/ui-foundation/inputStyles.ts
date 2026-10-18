// P1.1 文本输入 / 文本域 / Select 触发器的类名纯函数
// （spec: docs/specs/m4-ui-foundation.md §2.2）。只使用 DESIGN.md 的语义 token。

import type { RadiusLevel } from "../lib/uiFoundation.js";
import { controlRadiusClass } from "./radius.js";

export type FieldSize = "sm" | "default" | "lg";

const FIELD_BASE = [
  "w-full bg-input text-foreground border border-input-border",
  "placeholder:text-foreground-subtlest",
  "transition-colors",
  "hover:border-input-border-hover",
  "focus:border-input-border-focused focus:bg-input-focused focus:outline-none",
  "disabled:cursor-not-allowed disabled:opacity-50",
].join(" ");

const FIELD_INVALID = "border-destructive hover:border-destructive focus:border-destructive";

const FIELD_SIZES: Record<FieldSize, string> = {
  sm: "h-7 px-2.5 text-ui-sm",
  default: "h-8 px-3 text-ui-base",
  lg: "h-9 px-3.5 text-ui-base",
};

export interface FieldClassNameOptions {
  size?: FieldSize;
  parentRadius?: RadiusLevel;
  /** 仅真实校验错误时使用；配 aria-invalid。 */
  invalid?: boolean;
  className?: string;
}

function join(parts: Array<string | false | undefined>): string {
  return parts.filter(Boolean).join(" ");
}

export function inputClassName(options: FieldClassNameOptions = {}): string {
  const size = options.size ?? "default";
  return join([
    FIELD_BASE,
    FIELD_SIZES[size],
    controlRadiusClass(options.parentRadius),
    options.invalid && FIELD_INVALID,
    options.className,
  ]);
}

export function textareaClassName(options: FieldClassNameOptions = {}): string {
  return join([
    FIELD_BASE,
    "min-h-16 px-3 py-2 text-ui-base",
    controlRadiusClass(options.parentRadius),
    options.invalid && FIELD_INVALID,
    options.className,
  ]);
}

export interface SelectTriggerClassNameOptions extends FieldClassNameOptions {
  /** 展开态：高亮边框但不改背景（触发器仍像输入框）。 */
  open?: boolean;
}

export function selectTriggerClassName(options: SelectTriggerClassNameOptions = {}): string {
  const size = options.size ?? "default";
  return join([
    FIELD_BASE,
    "inline-flex items-center justify-between gap-2 text-left",
    FIELD_SIZES[size],
    "cursor-pointer",
    controlRadiusClass(options.parentRadius),
    options.open && "border-input-border-focused",
    options.invalid && FIELD_INVALID,
    options.className,
  ]);
}
