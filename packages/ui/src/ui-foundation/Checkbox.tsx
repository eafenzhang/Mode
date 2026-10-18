// P1.1 Checkbox（adapter map §1 缺口；原生 input，保留浏览器语义）。

import { forwardRef, type InputHTMLAttributes } from "react";

import { checkboxClassName } from "./formStyles.js";

export type CheckboxProps = Omit<InputHTMLAttributes<HTMLInputElement>, "type">;

export const Checkbox = forwardRef<HTMLInputElement, CheckboxProps>(function Checkbox(
  { className, ...rest },
  ref,
) {
  return <input {...rest} ref={ref} type="checkbox" className={checkboxClassName(className)} />;
});
