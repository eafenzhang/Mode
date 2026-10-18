// P1.1 InputGroup（spec: docs/specs/m4-ui-foundation.md §2.11）。
// 组内输入/文本域去掉独立边框与背景，由容器统一承担焦点态。

import {
  forwardRef,
  type HTMLAttributes,
  type InputHTMLAttributes,
  type TextareaHTMLAttributes,
} from "react";

import { Button, type ButtonProps } from "./Button.js";

const GROUP_CLASS =
  "flex min-w-0 items-center overflow-hidden rounded-md border border-input-border bg-input text-foreground " +
  "transition-colors focus-within:border-input-border-focused";

const FIELD_CLASS =
  "min-w-0 flex-1 bg-transparent text-ui-base text-foreground outline-none " +
  "placeholder:text-foreground-subtlest disabled:cursor-not-allowed disabled:opacity-50";

export interface InputGroupProps extends HTMLAttributes<HTMLDivElement> {}

export const InputGroup = forwardRef<HTMLDivElement, InputGroupProps>(function InputGroup(
  { className, ...rest },
  ref,
) {
  return (
    <div
      {...rest}
      ref={ref}
      data-slot="input-group"
      className={[GROUP_CLASS, className].filter(Boolean).join(" ")}
    />
  );
});

export type InputGroupAddonAlign = "inline-start" | "inline-end" | "block-start" | "block-end";

export interface InputGroupAddonProps extends HTMLAttributes<HTMLDivElement> {
  align?: InputGroupAddonAlign;
}

const ADDON_ALIGN_CLASS: Record<InputGroupAddonAlign, string> = {
  "inline-start": "order-first",
  "inline-end": "order-last",
  "block-start": "order-first self-start",
  "block-end": "order-last self-end",
};

export const InputGroupAddon = forwardRef<HTMLDivElement, InputGroupAddonProps>(
  function InputGroupAddon({ align = "inline-start", className, ...rest }, ref) {
    return (
      <div
        {...rest}
        ref={ref}
        data-align={align}
        className={[
          "flex shrink-0 items-center gap-1 px-1.5 text-foreground-subtle",
          ADDON_ALIGN_CLASS[align],
          className,
        ]
          .filter(Boolean)
          .join(" ")}
      />
    );
  },
);

export interface InputGroupButtonProps extends ButtonProps {}

export const InputGroupButton = forwardRef<HTMLButtonElement, InputGroupButtonProps>(
  function InputGroupButton({ className, variant = "ghost", size = "icon-xs", ...rest }, ref) {
    return (
      <Button
        {...rest}
        ref={ref}
        variant={variant}
        size={size}
        className={["rounded-sm", className].filter(Boolean).join(" ")}
      />
    );
  },
);

export interface InputGroupInputProps extends InputHTMLAttributes<HTMLInputElement> {}

export const InputGroupInput = forwardRef<HTMLInputElement, InputGroupInputProps>(
  function InputGroupInput({ className, ...rest }, ref) {
    return (
      <input
        {...rest}
        ref={ref}
        data-slot="input-group-input"
        className={[FIELD_CLASS, className].filter(Boolean).join(" ")}
      />
    );
  },
);

export interface InputGroupTextareaProps extends TextareaHTMLAttributes<HTMLTextAreaElement> {}

export const InputGroupTextarea = forwardRef<HTMLTextAreaElement, InputGroupTextareaProps>(
  function InputGroupTextarea({ className, ...rest }, ref) {
    return (
      <textarea
        {...rest}
        ref={ref}
        data-slot="input-group-textarea"
        className={[FIELD_CLASS, className].filter(Boolean).join(" ")}
      />
    );
  },
);

export interface InputGroupTextProps extends HTMLAttributes<HTMLSpanElement> {}

export const InputGroupText = forwardRef<HTMLSpanElement, InputGroupTextProps>(
  function InputGroupText({ className, ...rest }, ref) {
    return (
      <span
        {...rest}
        ref={ref}
        className={["text-ui-sm text-foreground-subtle", className].filter(Boolean).join(" ")}
      />
    );
  },
);
