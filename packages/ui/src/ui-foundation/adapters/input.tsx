// M4 adapter 第一批：Input（保留旧导出名）。

import { forwardRef, type InputHTMLAttributes } from "react";

import type { RadiusLevel } from "../../lib/uiFoundation.js";
import { Input as FoundationInput } from "../Input.js";
import { inputClassName, type FieldSize } from "../inputStyles.js";

export type AdapterInputSize = "xs" | "sm" | "default" | "lg";

export interface AdapterInputProps extends Omit<InputHTMLAttributes<HTMLInputElement>, "size"> {
  fieldSize?: FieldSize;
  parentRadius?: RadiusLevel;
  invalid?: boolean;
  /** 旧 API 的 size；xs 归一到 foundation 的 sm。 */
  size?: AdapterInputSize;
  /** 原生 size 属性（旧 API 用 htmlSize 避免与 cva size 冲突）。 */
  htmlSize?: number;
}

function mapInputSize(size: AdapterInputSize | undefined): FieldSize | undefined {
  if (size === undefined) {
    return undefined;
  }
  return size === "xs" ? "sm" : size;
}

export const Input = forwardRef<HTMLInputElement, AdapterInputProps>(function Input(
  { fieldSize, parentRadius, invalid, size, htmlSize, ...rest },
  ref,
) {
  return (
    <FoundationInput
      {...rest}
      ref={ref}
      fieldSize={fieldSize ?? mapInputSize(size)}
      parentRadius={parentRadius}
      invalid={invalid}
      size={htmlSize}
      data-slot="input"
    />
  );
});

export function inputVariants(
  options: {
    fieldSize?: FieldSize;
    parentRadius?: RadiusLevel;
    invalid?: boolean;
    size?: AdapterInputSize;
  } = {},
): string {
  return inputClassName({
    size: options.fieldSize ?? mapInputSize(options.size),
    parentRadius: options.parentRadius,
    invalid: options.invalid,
  });
}
