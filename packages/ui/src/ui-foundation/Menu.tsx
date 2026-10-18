// P1.1 Menu primitive（spec: docs/specs/m4-ui-foundation.md §2.4）。
// 菜单 shell rounded-lg；选项 rounded-md、紧凑行、2px 垂直间距由父级 gap-0.5 控制。

import { forwardRef, type ButtonHTMLAttributes, type HTMLAttributes, type ReactNode } from "react";

import { Separator } from "./Separator.js";
import { menuClassName, menuGroupClassName, menuItemClassName } from "./surfaceStyles.js";

export interface MenuProps extends HTMLAttributes<HTMLDivElement> {
  label?: string;
}

export const Menu = forwardRef<HTMLDivElement, MenuProps>(function Menu(
  { label, className, children, ...rest },
  ref,
) {
  return (
    <div {...rest} ref={ref} role="menu" aria-label={label} className={menuClassName(className)}>
      {children}
    </div>
  );
});

export interface MenuItemProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  destructive?: boolean;
}

export const MenuItem = forwardRef<HTMLButtonElement, MenuItemProps>(function MenuItem(
  { destructive, className, children, type, ...rest },
  ref,
) {
  return (
    <button
      {...rest}
      ref={ref}
      type={type ?? "button"}
      role="menuitem"
      className={menuItemClassName({ destructive, className })}
    >
      {children}
    </button>
  );
});

export interface MenuGroupProps extends HTMLAttributes<HTMLDivElement> {
  label?: string;
}

export const MenuGroup = forwardRef<HTMLDivElement, MenuGroupProps>(function MenuGroup(
  { label, className, ...rest },
  ref,
) {
  return (
    <div
      {...rest}
      ref={ref}
      role="group"
      aria-label={label}
      className={menuGroupClassName(className)}
    />
  );
});

export function MenuSeparator({ className }: { className?: string }): ReactNode {
  return <Separator className={className ? `my-1 ${className}` : "my-1"} />;
}

export interface MenuCheckboxItemProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  checked: boolean;
  destructive?: boolean;
}

export const MenuCheckboxItem = forwardRef<HTMLButtonElement, MenuCheckboxItemProps>(
  function MenuCheckboxItem({ checked, destructive, className, children, type, ...rest }, ref) {
    return (
      <button
        {...rest}
        ref={ref}
        type={type ?? "button"}
        role="menuitemcheckbox"
        aria-checked={checked}
        className={menuItemClassName({ destructive, className })}
      >
        <span aria-hidden="true" className="w-3 shrink-0">
          {checked ? "✓" : null}
        </span>
        {children}
      </button>
    );
  },
);

export interface MenuRadioItemProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  checked: boolean;
  destructive?: boolean;
}

export const MenuRadioItem = forwardRef<HTMLButtonElement, MenuRadioItemProps>(
  function MenuRadioItem({ checked, destructive, className, children, type, ...rest }, ref) {
    return (
      <button
        {...rest}
        ref={ref}
        type={type ?? "button"}
        role="menuitemradio"
        aria-checked={checked}
        className={menuItemClassName({ destructive, className })}
      >
        <span aria-hidden="true" className="flex w-3 shrink-0 justify-center">
          {checked ? <span className="size-1.5 rounded-full bg-current" /> : null}
        </span>
        {children}
      </button>
    );
  },
);
