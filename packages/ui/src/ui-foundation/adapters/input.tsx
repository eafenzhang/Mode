// M4 adapter 第一批：Input（保留旧导出名）。

import { forwardRef, type InputHTMLAttributes } from "react";

import type { RadiusLevel } from "../../lib/uiFoundation.js";
import { Input as FoundationInput } from "../Input.js";
import { inputClassName, type FieldSize } from "../inputStyles.js";

export interface AdapterInputProps extends InputHTMLAttributes<HTMLInputElement> {
  fieldSize?: FieldSize;
  parentRadius?: RadiusLevel;
  invalid?: boolean;
}

export const Input = forwardRef<HTMLInputElement, AdapterInputProps>(function Input(
  { fieldSize, parentRadius, invalid, ...rest },
  ref,
) {
  return (
    <FoundationInput
      {...rest}
      ref={ref}
      fieldSize={fieldSize}
      parentRadius={parentRadius}
      invalid={invalid}
    />
  );
});

export function inputVariants(
  options: { fieldSize?: FieldSize; parentRadius?: RadiusLevel; invalid?: boolean } = {},
): string {
  return inputClassName({
    size: options.fieldSize,
    parentRadius: options.parentRadius,
    invalid: options.invalid,
  });
}
