// P1.1 Input primitive（spec: docs/specs/m4-ui-foundation.md §2.2）。

import { forwardRef, type InputHTMLAttributes } from "react";

import type { RadiusLevel } from "../lib/uiFoundation.js";
import { inputClassName, type FieldSize } from "./inputStyles.js";

export interface InputProps extends InputHTMLAttributes<HTMLInputElement> {
  fieldSize?: FieldSize;
  parentRadius?: RadiusLevel;
  invalid?: boolean;
}

export const Input = forwardRef<HTMLInputElement, InputProps>(function Input(
  { fieldSize, parentRadius, invalid, className, ...rest },
  ref,
) {
  return (
    <input
      {...rest}
      ref={ref}
      aria-invalid={invalid || undefined}
      className={inputClassName({ size: fieldSize, parentRadius, invalid, className })}
    />
  );
});
