// M4 adapter 第二批：dropdown-menu 的纯展示子件（旧导出名）。
// 根 DropdownMenu / Trigger / Sub 的开关与定位语义需要按调用方用法补齐，未在此猜测。

import { forwardRef, type HTMLAttributes, type ReactNode } from "react";

import {
  MenuCheckboxItem as DropdownMenuCheckboxItem,
  MenuGroup as DropdownMenuGroup,
  MenuItem as DropdownMenuItem,
  MenuRadioItem as DropdownMenuRadioItem,
  MenuSeparator as DropdownMenuSeparator,
} from "../Menu.js";
import { menuClassName, menuLabelClassName } from "../surfaceStyles.js";

export {
  DropdownMenuCheckboxItem,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
};

export const DropdownMenuContent = forwardRef<HTMLDivElement, HTMLAttributes<HTMLDivElement>>(
  function DropdownMenuContent({ className, ...rest }, ref) {
    return <div {...rest} ref={ref} role="menu" className={menuClassName(className)} />;
  },
);

export function DropdownMenuLabel({
  className,
  children,
}: HTMLAttributes<HTMLDivElement>): ReactNode {
  return <div className={menuLabelClassName(className)}>{children}</div>;
}
