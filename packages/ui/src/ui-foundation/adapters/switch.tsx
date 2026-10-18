// M4 adapter：Switch（旧导出名；兼容可选 label/aria-label）。

import { forwardRef, type ButtonHTMLAttributes } from "react";

import { Switch as FoundationSwitch } from "../Switch.js";

export interface SwitchProps extends Omit<
  ButtonHTMLAttributes<HTMLButtonElement>,
  "onChange" | "type"
> {
  checked?: boolean;
  onCheckedChange?: (checked: boolean) => void;
  label?: string;
  /** 旧 API 尺寸；当前只接受 sm/default，视觉差异登记。 */
  size?: "sm" | "default";
}

export const Switch = forwardRef<HTMLButtonElement, SwitchProps>(function Switch(
  { checked, onCheckedChange, label, size: _size, "aria-label": ariaLabel, ...rest },
  ref,
) {
  return (
    <FoundationSwitch
      {...rest}
      ref={ref}
      checked={checked ?? false}
      onCheckedChange={onCheckedChange}
      label={label ?? ariaLabel}
    />
  );
});
