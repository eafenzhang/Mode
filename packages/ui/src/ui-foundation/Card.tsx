// P1.1 Card primitive（spec: docs/specs/m4-ui-foundation.md §2.3）。

import { forwardRef, type HTMLAttributes } from "react";

import type { RadiusLevel } from "../lib/uiFoundation.js";
import { cardClassName } from "./surfaceStyles.js";

export interface CardProps extends HTMLAttributes<HTMLDivElement> {
  parentRadius?: RadiusLevel;
  lowEmphasis?: boolean;
  selected?: boolean;
  padded?: boolean;
}

export const Card = forwardRef<HTMLDivElement, CardProps>(function Card(
  { parentRadius, lowEmphasis, selected, padded, className, ...rest },
  ref,
) {
  return (
    <div
      {...rest}
      ref={ref}
      className={cardClassName({ parentRadius, lowEmphasis, selected, padded, className })}
    />
  );
});
