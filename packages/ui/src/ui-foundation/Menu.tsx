// P1.1 Menu primitive（spec: docs/specs/m4-ui-foundation.md §2.4）。
// 菜单 shell rounded-lg；选项 rounded-md、紧凑行、2px 垂直间距由父级 gap-0.5 控制。

import { forwardRef, type ButtonHTMLAttributes, type HTMLAttributes } from "react";

import { menuClassName, menuItemClassName } from "./surfaceStyles.js";

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
