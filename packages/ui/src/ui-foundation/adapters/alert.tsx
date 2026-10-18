// M4 adapter：Alert（旧 variant → 新 tone）。

import { forwardRef, type HTMLAttributes } from "react";

import { Alert as FoundationAlert } from "../Alert.js";
import type { AlertTone } from "../alertStyles.js";

export { AlertAction, AlertDescription, AlertTitle } from "../Alert.js";

export type AdapterAlertVariant = "default" | "destructive" | "success" | "warning";

export function mapAlertVariant(variant: AdapterAlertVariant = "default"): AlertTone {
  switch (variant) {
    case "destructive":
      return "destructive";
    case "success":
      return "success";
    case "warning":
      return "warning";
    case "default":
      return "neutral";
  }
}

export interface AdapterAlertProps extends HTMLAttributes<HTMLDivElement> {
  variant?: AdapterAlertVariant;
}

export const Alert = forwardRef<HTMLDivElement, AdapterAlertProps>(function Alert(
  { variant, ...rest },
  ref,
) {
  return <FoundationAlert {...rest} ref={ref} tone={mapAlertVariant(variant)} />;
});
