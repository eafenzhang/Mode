// P1.1 Switch（adapter map §1 缺口；button + role="switch"）。

import { forwardRef, type ButtonHTMLAttributes } from "react";

import { switchThumbClassName, switchTrackClassName } from "./formStyles.js";

export interface SwitchProps
  extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "onChange"> {
  checked: boolean;
  onCheckedChange?: (checked: boolean) => void;
  /** 可访问名称（i18n）；缺省时由调用方通过 aria-label 提供。 */
  label?: string;
}

export const Switch = forwardRef<HTMLButtonElement, SwitchProps>(function Switch(
  { checked, onCheckedChange, label, className, type, disabled, onClick, ...rest },
  ref,
) {
  return (
    <button
      {...rest}
      ref={ref}
      type={type ?? "button"}
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={(event) => {
        onClick?.(event);
        if (!event.defaultPrevented) {
          onCheckedChange?.(!checked);
        }
      }}
      className={switchTrackClassName(checked, className)}
    >
      <span aria-hidden="true" className={switchThumbClassName(checked)} />
    </button>
  );
});
