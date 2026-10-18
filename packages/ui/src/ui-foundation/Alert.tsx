// P1.1 Alert 组件（adapter map §1 缺口）。

import { forwardRef, type HTMLAttributes } from "react";

import {
  alertActionClassName,
  alertClassName,
  alertDescriptionClassName,
  alertTitleClassName,
  type AlertTone,
} from "./alertStyles.js";

export interface AlertProps extends HTMLAttributes<HTMLDivElement> {
  tone?: AlertTone;
}

export const Alert = forwardRef<HTMLDivElement, AlertProps>(function Alert(
  { tone, className, ...rest },
  ref,
) {
  const assertive = tone === "destructive" || tone === "warning";
  return (
    <div
      {...rest}
      ref={ref}
      role={assertive ? "alert" : "status"}
      className={alertClassName({ tone, className })}
    />
  );
});

export const AlertTitle = forwardRef<HTMLDivElement, HTMLAttributes<HTMLDivElement>>(
  function AlertTitle({ className, ...rest }, ref) {
    return <div {...rest} ref={ref} className={alertTitleClassName(className)} />;
  },
);

export const AlertDescription = forwardRef<HTMLDivElement, HTMLAttributes<HTMLDivElement>>(
  function AlertDescription({ className, ...rest }, ref) {
    return <div {...rest} ref={ref} className={alertDescriptionClassName(className)} />;
  },
);

export const AlertAction = forwardRef<HTMLDivElement, HTMLAttributes<HTMLDivElement>>(
  function AlertAction({ className, ...rest }, ref) {
    return <div {...rest} ref={ref} className={alertActionClassName(className)} />;
  },
);
