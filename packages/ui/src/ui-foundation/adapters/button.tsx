// M4 adapter 第一批：Button（保留旧导出名，映射到 ui-foundation）。
// 旧 props 的完整集合需在逐屏切换时核对；这里覆盖 DESIGN.md 与旧命名中已知的
// variant/size 子集，未知值回落到默认。

import { forwardRef, type ButtonHTMLAttributes } from "react";

import type { RadiusLevel } from "../../lib/uiFoundation.js";
import { Button as FoundationButton } from "../Button.js";
import { buttonClassName, type ButtonSize, type ButtonVariant } from "../buttonStyles.js";

export type AdapterButtonVariant =
  | "default"
  | "primary"
  | "outline"
  | "secondary"
  | "ghost"
  | "destructive"
  | "warning"
  | "link";

/** foundation 没有 warning，adapter 用 secondary 叠加语义色类。 */
const WARNING_VARIANT_CLASS =
  "bg-warning text-warning-foreground hover:bg-warning/90 focus-visible:border-warning/40 focus-visible:ring-warning/20";

export type AdapterButtonSize =
  | "default"
  | "xs"
  | "sm"
  | "lg"
  | "icon"
  | "icon-md"
  | "icon-xs"
  | "icon-sm"
  | "icon-lg";

export function mapButtonVariant(variant: AdapterButtonVariant = "default"): ButtonVariant {
  if (variant === "default") {
    return "primary";
  }
  return variant === "warning" ? "secondary" : variant;
}

export function mapButtonSize(size: AdapterButtonSize = "default"): ButtonSize {
  return size === "icon-md" ? "icon" : size;
}

export interface AdapterButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: AdapterButtonVariant;
  size?: AdapterButtonSize;
  parentRadius?: RadiusLevel;
  loading?: boolean;
}

export const Button = forwardRef<HTMLButtonElement, AdapterButtonProps>(function Button(
  { variant = "default", size = "default", parentRadius, loading, className, ...rest },
  ref,
) {
  const mergedClassName = [className, variant === "warning" ? WARNING_VARIANT_CLASS : ""]
    .filter(Boolean)
    .join(" ");
  return (
    <FoundationButton
      {...rest}
      ref={ref}
      variant={mapButtonVariant(variant)}
      size={mapButtonSize(size)}
      parentRadius={parentRadius}
      loading={loading}
      className={mergedClassName}
      data-slot="button"
      data-variant={variant}
      data-size={size}
    />
  );
});

export function buttonVariants(
  options: {
    variant?: AdapterButtonVariant;
    size?: AdapterButtonSize;
    parentRadius?: RadiusLevel;
    className?: string;
  } = {},
): string {
  const warning = options.variant === "warning";
  return buttonClassName({
    variant: mapButtonVariant(options.variant),
    size: mapButtonSize(options.size),
    parentRadius: options.parentRadius,
    className: [options.className, warning ? WARNING_VARIANT_CLASS : ""].filter(Boolean).join(" "),
  });
}
