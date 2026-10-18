// P1.1 Avatar（adapter map §1 缺口；静态结构，img/fallback 由调用方组合）。

import { forwardRef, type HTMLAttributes, type ImgHTMLAttributes, type ReactNode } from "react";

import {
  avatarBadgeClassName,
  avatarClassName,
  avatarFallbackClassName,
  avatarGroupClassName,
  avatarImageClassName,
  type AvatarBadgeTone,
  type AvatarSize,
} from "./avatarStyles.js";

export interface AvatarProps extends HTMLAttributes<HTMLSpanElement> {
  size?: AvatarSize;
}

export const Avatar = forwardRef<HTMLSpanElement, AvatarProps>(function Avatar(
  { size, className, ...rest },
  ref,
) {
  return <span {...rest} ref={ref} className={avatarClassName(size, className)} />;
});

export type AvatarImageProps = ImgHTMLAttributes<HTMLImageElement>;

export const AvatarImage = forwardRef<HTMLImageElement, AvatarImageProps>(function AvatarImage(
  { className, alt = "", ...rest },
  ref,
) {
  return <img {...rest} ref={ref} alt={alt} className={avatarImageClassName(className)} />;
});

export interface AvatarFallbackProps extends HTMLAttributes<HTMLSpanElement> {
  children?: ReactNode;
}

export const AvatarFallback = forwardRef<HTMLSpanElement, AvatarFallbackProps>(
  function AvatarFallback({ className, ...rest }, ref) {
    return <span {...rest} ref={ref} className={avatarFallbackClassName(className)} />;
  },
);

export interface AvatarBadgeProps extends HTMLAttributes<HTMLSpanElement> {
  tone?: AvatarBadgeTone;
}

export const AvatarBadge = forwardRef<HTMLSpanElement, AvatarBadgeProps>(function AvatarBadge(
  { tone, className, ...rest },
  ref,
) {
  return (
    <span
      {...rest}
      ref={ref}
      aria-hidden="true"
      className={avatarBadgeClassName(tone, className)}
    />
  );
});

export interface AvatarGroupProps extends HTMLAttributes<HTMLSpanElement> {
  children?: ReactNode;
}

export const AvatarGroup = forwardRef<HTMLSpanElement, AvatarGroupProps>(function AvatarGroup(
  { className, ...rest },
  ref,
) {
  return <span {...rest} ref={ref} className={avatarGroupClassName(className)} />;
});

export interface AvatarGroupCountProps extends HTMLAttributes<HTMLSpanElement> {
  count: number;
  size?: AvatarSize;
}

export const AvatarGroupCount = forwardRef<HTMLSpanElement, AvatarGroupCountProps>(
  function AvatarGroupCount({ count, size, className, ...rest }, ref) {
    return (
      <span
        {...rest}
        ref={ref}
        aria-label={`+${count}`}
        className={avatarClassName(size, className)}
      >
        +{count}
      </span>
    );
  },
);
