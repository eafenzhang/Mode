// P1.1 composable 补漏：Select 内容区/选项/组/值（adapter map §5）。
// 触发器见 SelectTrigger.tsx；展开面板沿用菜单 shell 语言。

import {
  forwardRef,
  type ButtonHTMLAttributes,
  type HTMLAttributes,
  type ReactNode,
} from "react";

import {
  selectContentClassName,
  selectGroupLabelClassName,
  selectItemClassName,
  selectValueClassName,
} from "./selectStyles.js";

export interface SelectContentProps extends HTMLAttributes<HTMLDivElement> {
  label?: string;
}

export const SelectContent = forwardRef<HTMLDivElement, SelectContentProps>(
  function SelectContent({ label, className, ...rest }, ref) {
    return (
      <div
        {...rest}
        ref={ref}
        role="listbox"
        aria-label={label}
        className={selectContentClassName(className)}
      />
    );
  },
);

export interface SelectItemProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  selected?: boolean;
}

export const SelectItem = forwardRef<HTMLButtonElement, SelectItemProps>(
  function SelectItem({ selected, className, type, children, ...rest }, ref) {
    return (
      <button
        {...rest}
        ref={ref}
        type={type ?? "button"}
        role="option"
        aria-selected={selected}
        className={selectItemClassName(className)}
      >
        {children}
      </button>
    );
  },
);

export interface SelectGroupProps extends HTMLAttributes<HTMLDivElement> {
  label?: string;
}

export const SelectGroup = forwardRef<HTMLDivElement, SelectGroupProps>(
  function SelectGroup({ label, className, ...rest }, ref) {
    return <div {...rest} ref={ref} role="group" aria-label={label} className={className} />;
  },
);

export function SelectGroupLabel({
  className,
  children,
}: HTMLAttributes<HTMLDivElement>): ReactNode {
  return <div className={selectGroupLabelClassName(className)}>{children}</div>;
}

export interface SelectValueProps extends HTMLAttributes<HTMLSpanElement> {
  placeholder?: boolean;
}

export const SelectValue = forwardRef<HTMLSpanElement, SelectValueProps>(
  function SelectValue({ placeholder, className, ...rest }, ref) {
    return <span {...rest} ref={ref} className={selectValueClassName({ placeholder, className })} />;
  },
);
