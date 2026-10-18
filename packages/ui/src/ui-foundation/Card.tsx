// P1.1 Card primitive（spec: docs/specs/m4-ui-foundation.md §2.3）。

import { forwardRef, type HTMLAttributes } from "react";

import type { RadiusLevel } from "../lib/uiFoundation.js";
import {
  cardActionClassName,
  cardClassName,
  cardContentClassName,
  cardDescriptionClassName,
  cardFooterClassName,
  cardHeaderClassName,
  cardTitleClassName,
} from "./surfaceStyles.js";

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

export const CardHeader = forwardRef<HTMLDivElement, HTMLAttributes<HTMLDivElement>>(
  function CardHeader({ className, ...rest }, ref) {
    return <div {...rest} ref={ref} className={cardHeaderClassName(className)} />;
  },
);

export const CardTitle = forwardRef<HTMLDivElement, HTMLAttributes<HTMLDivElement>>(
  function CardTitle({ className, ...rest }, ref) {
    return <div {...rest} ref={ref} className={cardTitleClassName(className)} />;
  },
);

export const CardDescription = forwardRef<HTMLDivElement, HTMLAttributes<HTMLDivElement>>(
  function CardDescription({ className, ...rest }, ref) {
    return <div {...rest} ref={ref} className={cardDescriptionClassName(className)} />;
  },
);

export const CardContent = forwardRef<HTMLDivElement, HTMLAttributes<HTMLDivElement>>(
  function CardContent({ className, ...rest }, ref) {
    return <div {...rest} ref={ref} className={cardContentClassName(className)} />;
  },
);

export const CardFooter = forwardRef<HTMLDivElement, HTMLAttributes<HTMLDivElement>>(
  function CardFooter({ className, ...rest }, ref) {
    return <div {...rest} ref={ref} className={cardFooterClassName(className)} />;
  },
);

export const CardAction = forwardRef<HTMLDivElement, HTMLAttributes<HTMLDivElement>>(
  function CardAction({ className, ...rest }, ref) {
    return <div {...rest} ref={ref} className={cardActionClassName(className)} />;
  },
);
