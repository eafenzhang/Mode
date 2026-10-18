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
  | "link";

export type AdapterButtonSize =
  | "default"
  | "xs"
  | "sm"
  | "lg"
  | "icon"
  | "icon-xs"
  | "icon-sm"
  | "icon-lg";

export function mapButtonVariant(variant: AdapterButtonVariant = "default"): ButtonVariant {
  return variant === "default" ? "primary" : variant;
}

export function mapButtonSize(size: AdapterButtonSize = "default"): ButtonSize {
  return size;
}

export interface AdapterButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: AdapterButtonVariant;
  size?: AdapterButtonSize;
  parentRadius?: RadiusLevel;
  loading?: boolean;
}

export const Button = forwardRef<HTMLButtonElement, AdapterButtonProps>(function Button(
  { variant, size, parentRadius, loading, ...rest },
  ref,
) {
  return (
    <FoundationButton
      {...rest}
      ref={ref}
      variant={mapButtonVariant(variant)}
      size={mapButtonSize(size)}
      parentRadius={parentRadius}
      loading={loading}
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
  return buttonClassName({
    variant: mapButtonVariant(options.variant),
    size: mapButtonSize(options.size),
    parentRadius: options.parentRadius,
    className: options.className,
  });
}
