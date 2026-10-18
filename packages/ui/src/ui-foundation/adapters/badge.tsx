// M4 adapter 第一批：Badge（旧 variant → 新 tone）。

import { forwardRef, type HTMLAttributes } from "react";

import { Badge as FoundationBadge } from "../Badge.js";
import { badgeClassName, type BadgeTone } from "../badgeStyles.js";

export type AdapterBadgeVariant =
  | "default"
  | "secondary"
  | "outline"
  | "destructive"
  | "success"
  | "warning";

export function mapBadgeVariant(variant: AdapterBadgeVariant = "default"): BadgeTone {
  switch (variant) {
    case "destructive":
      return "destructive";
    case "success":
      return "success";
    case "warning":
      return "warning";
    case "default":
    case "secondary":
    case "outline":
      return "neutral";
  }
}

export interface AdapterBadgeProps extends HTMLAttributes<HTMLSpanElement> {
  variant?: AdapterBadgeVariant;
  pill?: boolean;
}

export const Badge = forwardRef<HTMLSpanElement, AdapterBadgeProps>(function Badge(
  { variant, pill, ...rest },
  ref,
) {
  return <FoundationBadge {...rest} ref={ref} tone={mapBadgeVariant(variant)} pill={pill} />;
});

export function badgeVariants(
  options: { variant?: AdapterBadgeVariant; pill?: boolean; className?: string } = {},
): string {
  return badgeClassName({
    tone: mapBadgeVariant(options.variant),
    pill: options.pill,
    className: options.className,
  });
}
