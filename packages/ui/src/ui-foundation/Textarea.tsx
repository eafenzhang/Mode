// P1.1 Textarea primitive（spec: docs/specs/m4-ui-foundation.md §2.2）。

import { forwardRef, type TextareaHTMLAttributes } from "react";

import type { RadiusLevel } from "../lib/uiFoundation.js";
import { textareaClassName } from "./inputStyles.js";

export interface TextareaProps extends TextareaHTMLAttributes<HTMLTextAreaElement> {
  parentRadius?: RadiusLevel;
  invalid?: boolean;
}

export const Textarea = forwardRef<HTMLTextAreaElement, TextareaProps>(function Textarea(
  { parentRadius, invalid, className, ...rest },
  ref,
) {
  return (
    <textarea
      {...rest}
      ref={ref}
      aria-invalid={invalid || undefined}
      className={textareaClassName({ parentRadius, invalid, className })}
    />
  );
});
