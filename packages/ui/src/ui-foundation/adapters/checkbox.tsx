// M4 adapter：Checkbox（旧 API：checked 三态 + onCheckedChange）。

import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useRef,
  type InputHTMLAttributes,
} from "react";

import { checkboxClassName } from "../formStyles.js";

export type AdapterCheckboxChecked = boolean | "indeterminate";

export interface CheckboxProps
  extends Omit<InputHTMLAttributes<HTMLInputElement>, "checked" | "type" | "onChange"> {
  checked?: AdapterCheckboxChecked;
  onCheckedChange?: (checked: boolean | "indeterminate") => void;
  /** 旧 API 的勾选图标线宽；native checkbox 不使用，接受后忽略。 */
  checkIconStrokeWidth?: number;
}

export const Checkbox = forwardRef<HTMLInputElement, CheckboxProps>(function Checkbox(
  { checked, onCheckedChange, checkIconStrokeWidth: _strokeWidth, className, ...rest },
  ref,
) {
  const innerRef = useRef<HTMLInputElement | null>(null);
  useImperativeHandle(ref, () => innerRef.current as HTMLInputElement, []);

  useEffect(() => {
    if (innerRef.current) {
      innerRef.current.indeterminate = checked === "indeterminate";
    }
  }, [checked]);

  return (
    <input
      {...rest}
      ref={innerRef}
      type="checkbox"
      checked={checked === true}
      onChange={(event) => onCheckedChange?.(event.target.checked)}
      className={checkboxClassName(className)}
    />
  );
});
