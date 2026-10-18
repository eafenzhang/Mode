// P1.1 RadioGroup / Radio（adapter map §1 缺口；原生 radio 语义）。

import { forwardRef, type HTMLAttributes, type InputHTMLAttributes } from "react";

import { radioClassName, radioGroupClassName } from "./formStyles.js";

export interface RadioGroupProps extends HTMLAttributes<HTMLDivElement> {
  label: string;
}

export const RadioGroup = forwardRef<HTMLDivElement, RadioGroupProps>(function RadioGroup(
  { label, className, ...rest },
  ref,
) {
  return (
    <div
      {...rest}
      ref={ref}
      role="radiogroup"
      aria-label={label}
      className={radioGroupClassName(className)}
    />
  );
});

export type RadioProps = Omit<InputHTMLAttributes<HTMLInputElement>, "type">;

export const Radio = forwardRef<HTMLInputElement, RadioProps>(function Radio(
  { className, ...rest },
  ref,
) {
  return <input {...rest} ref={ref} type="radio" className={radioClassName(className)} />;
});
