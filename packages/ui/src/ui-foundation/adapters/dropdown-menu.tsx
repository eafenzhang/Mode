// M4 adapter：DropdownMenu 根/触发器/内容与条目（旧 Radix API 形状）。
// 子菜单在 dropdown-submenu.tsx；定位与 Popover 共用同一套数学。

import {
  cloneElement,
  forwardRef,
  isValidElement,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ButtonHTMLAttributes,
  type HTMLAttributes,
  type MouseEvent as ReactMouseEvent,
  type ReactElement,
  type ReactNode,
} from "react";

import {
  MenuCheckboxItem,
  MenuGroup as DropdownMenuGroup,
  MenuItem,
  MenuRadioItem,
  MenuSeparator as DropdownMenuSeparatorImpl,
} from "../Menu.js";
import { menuLabelClassName } from "../surfaceStyles.js";
import { childRefOf, mergeRefs } from "./domTargets.js";
import { DropdownMenuContent } from "./dropdownMenuContent.js";
import {
  DropdownMenuAdapterContext,
  DropdownRadioGroupAdapterContext,
  menuSelectEvent,
  useDropdownMenuAdapterState,
  useDropdownRadioGroupAdapterState,
  type MenuSelectEvent,
} from "./dropdownMenuContext.js";
import { cn } from "@/components/lib/utils.js";

type ElementProps = Record<string, unknown>;

const useIsomorphicLayoutEffect = typeof window === "undefined" ? useEffect : useLayoutEffect;

export { DropdownMenuGroup, DropdownMenuContent };
export { type DropdownMenuContentProps } from "./dropdownMenuContent.js";

export function DropdownMenuSeparator({
  className,
}: {
  className?: string;
  key?: string;
}): ReactNode {
  return <DropdownMenuSeparatorImpl className={className} />;
}

export interface DropdownMenuProps {
  open?: boolean;
  defaultOpen?: boolean;
  onOpenChange?: (open: boolean) => void;
  modal?: boolean;
  children?: ReactNode;
}

export function DropdownMenu({
  open,
  defaultOpen,
  onOpenChange,
  modal: _modal,
  children,
}: DropdownMenuProps): ReactNode {
  const [uncontrolled, setUncontrolled] = useState(defaultOpen ?? false);
  // 根菜单内唯一打开的子菜单；指针进入另一个子菜单时由它把旧内容关掉。
  const [activeSubmenuId, setActiveSubmenuIdState] = useState<string | null>(null);
  const current = open ?? uncontrolled;
  const triggerRef = useRef<HTMLElement | null>(null);
  const contentId = useId();
  const setActiveSubmenuId = useCallback((id: string | null) => {
    setActiveSubmenuIdState(id);
  }, []);
  const setOpen = useCallback(
    (next: boolean) => {
      if (open === undefined) {
        setUncontrolled(next);
      }
      if (!next) {
        setActiveSubmenuIdState(null);
      }
      onOpenChange?.(next);
    },
    [open, onOpenChange],
  );
  const closeMenu = useCallback(() => {
    setOpen(false);
    triggerRef.current?.focus();
  }, [setOpen]);
  const state = useMemo(
    () => ({
      open: current,
      setOpen,
      closeMenu,
      triggerRef,
      contentId,
      activeSubmenuId,
      setActiveSubmenuId,
    }),
    [current, setOpen, closeMenu, contentId, activeSubmenuId, setActiveSubmenuId],
  );
  return (
    <DropdownMenuAdapterContext.Provider value={state}>
      {children}
    </DropdownMenuAdapterContext.Provider>
  );
}

export interface DropdownMenuTriggerProps extends HTMLAttributes<HTMLElement> {
  asChild?: boolean;
  disabled?: boolean;
}

export const DropdownMenuTrigger = forwardRef<HTMLElement, DropdownMenuTriggerProps>(
  function DropdownMenuTrigger({ asChild, disabled, children, onClick, ...rest }, ref) {
    const state = useDropdownMenuAdapterState();
    const handleClick = (event: ReactMouseEvent<HTMLElement>): void => {
      onClick?.(event);
      if (!event.defaultPrevented && !disabled) {
        state?.setOpen(!state?.open);
      }
    };
    const triggerProps = {
      "aria-expanded": state?.open ?? false,
      "aria-controls": state?.open ? state.contentId : undefined,
      "aria-haspopup": "menu" as const,
      "data-state": state?.open ? "open" : "closed",
      "aria-disabled": disabled || undefined,
    };
    if (asChild && isValidElement(children)) {
      const child = children as ReactElement<ElementProps>;
      const childProps = child.props;
      return cloneElement(child, {
        ...triggerProps,
        onClick: (event: ReactMouseEvent<HTMLElement>) => {
          (childProps.onClick as ((event: ReactMouseEvent<HTMLElement>) => void) | undefined)?.(
            event,
          );
          handleClick(event);
        },
        ref: mergeRefs(ref, state?.triggerRef, childRefOf<HTMLElement>(child)),
      });
    }
    return (
      <button
        {...rest}
        {...triggerProps}
        ref={mergeRefs(ref, state?.triggerRef)}
        type="button"
        disabled={disabled}
        onClick={handleClick}
      >
        {children}
      </button>
    );
  },
);

export function DropdownMenuPortal({ children }: { children?: ReactNode }): ReactNode {
  return children;
}

function selectAndClose(
  state: ReturnType<typeof useDropdownMenuAdapterState>,
  onSelect: ((event: MenuSelectEvent) => void) | undefined,
  extra?: () => void,
): void {
  const event = menuSelectEvent();
  onSelect?.(event);
  extra?.();
  if (!event.defaultPrevented) {
    state?.closeMenu();
  }
}

export interface DropdownMenuItemProps extends Omit<
  ButtonHTMLAttributes<HTMLButtonElement>,
  "onSelect"
> {
  inset?: boolean;
  variant?: "default" | "destructive";
  onSelect?: (event: MenuSelectEvent) => void;
}

export const DropdownMenuItem = forwardRef<HTMLButtonElement, DropdownMenuItemProps>(
  function DropdownMenuItem(
    { inset, variant = "default", onSelect, className, onClick, children, ...rest },
    ref,
  ) {
    const state = useDropdownMenuAdapterState();
    const merged = cn([className, inset ? "ps-7.5" : ""]);
    return (
      <MenuItem
        {...rest}
        ref={ref}
        destructive={variant === "destructive"}
        data-inset={inset ? "" : undefined}
        data-variant={variant}
        className={merged}
        onClick={(event) => {
          onClick?.(event);
          if (!event.defaultPrevented) {
            selectAndClose(state, onSelect);
          }
        }}
      >
        {children}
      </MenuItem>
    );
  },
);

export interface DropdownMenuCheckboxItemProps extends Omit<
  ButtonHTMLAttributes<HTMLButtonElement>,
  "onSelect"
> {
  checked?: boolean | "indeterminate";
  onCheckedChange?: (checked: boolean) => void;
  onSelect?: (event: MenuSelectEvent) => void;
  inset?: boolean;
  /** 选中标记位置；默认前置，工作区菜单按原版口径用行尾。 */
  indicatorPosition?: "start" | "end";
}

export const DropdownMenuCheckboxItem = forwardRef<
  HTMLButtonElement,
  DropdownMenuCheckboxItemProps
>(function DropdownMenuCheckboxItem(
  {
    checked = false,
    onCheckedChange,
    onSelect,
    inset,
    indicatorPosition,
    className,
    onClick,
    children,
    ...rest
  },
  ref,
) {
  const state = useDropdownMenuAdapterState();
  return (
    <MenuCheckboxItem
      {...rest}
      ref={ref}
      checked={checked === true}
      indicatorPosition={indicatorPosition}
      data-inset={inset ? "" : undefined}
      className={className}
      onClick={(event) => {
        onClick?.(event);
        if (event.defaultPrevented) {
          return;
        }
        onCheckedChange?.(checked !== true);
        selectAndClose(state, onSelect);
      }}
    >
      {children}
    </MenuCheckboxItem>
  );
});

export interface DropdownMenuRadioGroupProps extends HTMLAttributes<HTMLDivElement> {
  value?: string;
  onValueChange?: (value: string) => void;
}

export function DropdownMenuRadioGroup({
  value,
  onValueChange,
  children,
  ...rest
}: DropdownMenuRadioGroupProps): ReactNode {
  const state = useMemo(
    () => ({ value, setValue: (next: string) => onValueChange?.(next) }),
    [value, onValueChange],
  );
  return (
    <DropdownRadioGroupAdapterContext.Provider value={state}>
      <div {...rest} role="group">
        {children}
      </div>
    </DropdownRadioGroupAdapterContext.Provider>
  );
}

export interface DropdownMenuRadioItemProps extends Omit<
  ButtonHTMLAttributes<HTMLButtonElement>,
  "onSelect" | "value"
> {
  value: string;
  inset?: boolean;
  onSelect?: (event: MenuSelectEvent) => void;
}

export const DropdownMenuRadioItem = forwardRef<HTMLButtonElement, DropdownMenuRadioItemProps>(
  function DropdownMenuRadioItem(
    { value, inset, onSelect, className, onClick, children, ...rest },
    ref,
  ) {
    const menu = useDropdownMenuAdapterState();
    const group = useDropdownRadioGroupAdapterState();
    return (
      <MenuRadioItem
        {...rest}
        ref={ref}
        checked={group?.value === value}
        data-inset={inset ? "" : undefined}
        className={className}
        onClick={(event) => {
          onClick?.(event);
          if (event.defaultPrevented) {
            return;
          }
          group?.setValue(value);
          selectAndClose(menu, onSelect);
        }}
      >
        {children}
      </MenuRadioItem>
    );
  },
);

export interface DropdownMenuLabelProps extends HTMLAttributes<HTMLDivElement> {
  inset?: boolean;
}

export function DropdownMenuLabel({
  inset,
  className,
  children,
  ...rest
}: DropdownMenuLabelProps): ReactNode {
  const merged = cn([menuLabelClassName(className), inset ? "ps-7.5" : ""]);
  return (
    <div {...rest} data-inset={inset ? "" : undefined} className={merged}>
      {children}
    </div>
  );
}

export function DropdownMenuShortcut({
  className,
  ...rest
}: HTMLAttributes<HTMLSpanElement>): ReactNode {
  return (
    <span {...rest} className={cn(["ms-auto text-ui-xs tracking-widest opacity-60", className])} />
  );
}

export {
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  type DropdownMenuSubContentProps,
  type DropdownMenuSubProps,
  type DropdownMenuSubTriggerProps,
} from "./dropdown-submenu.js";
