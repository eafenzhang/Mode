// M4 adapter：Tabs（旧 API：Tabs/TabsList/TabsTrigger/TabsContent/tabsListVariants）。
// 旧调用点使用 Radix 语义：variant、asChild、forceMount 与 data-state/data-active。

import {
  cloneElement,
  forwardRef,
  isValidElement,
  useCallback,
  useMemo,
  useState,
  type ButtonHTMLAttributes,
  type HTMLAttributes,
  type MouseEvent,
  type ReactElement,
  type ReactNode,
  type Ref,
} from "react";

import { tabListClassName } from "../feedbackStyles.js";
import { Tab, Tabs as FoundationTabs } from "../Tabs.js";
import { childRefOf, mergeRefs } from "./domTargets.js";
import { TabsAdapterContext, useTabsAdapterState } from "./tabsContext.js";
import { cn } from "@/components/lib/utils.js";

type ElementProps = Record<string, unknown>;

export interface TabsProps extends HTMLAttributes<HTMLDivElement> {
  value?: string;
  defaultValue?: string;
  onValueChange?: (value: string) => void;
}

export function Tabs({
  value,
  defaultValue,
  onValueChange,
  className,
  children,
  ...rest
}: TabsProps): ReactNode {
  const [uncontrolled, setUncontrolled] = useState(defaultValue);
  const current = value ?? uncontrolled;
  const setValue = useCallback(
    (next: string) => {
      if (value === undefined) {
        setUncontrolled(next);
      }
      onValueChange?.(next);
    },
    [value, onValueChange],
  );
  const context = useMemo(() => ({ value: current, setValue }), [current, setValue]);
  return (
    <TabsAdapterContext.Provider value={context}>
      {/* 旧 Radix 根节点自带 group/tabs + flex（横向为 flex-col）。侧栏、终端等调用方
          在这层下面用 flex-1 + h-full 分配高度；根节点退化成普通 block 后整条高度链
          会变成 auto，滚动容器不再产生纵向滚动。 */}
      <div
        {...rest}
        data-slot="tabs"
        data-orientation="horizontal"
        className={cn("group/tabs flex data-horizontal:flex-col", className)}
      >
        {children}
      </div>
    </TabsAdapterContext.Provider>
  );
}

export interface TabsListProps extends HTMLAttributes<HTMLDivElement> {
  variant?: "default" | "line";
  label?: string;
}

export function tabsListVariants(variant: "default" | "line" = "default"): string {
  return variant === "line" ? `${tabListClassName()} gap-1 bg-transparent` : tabListClassName();
}

export const TabsList = forwardRef<HTMLDivElement, TabsListProps>(function TabsList(
  { variant = "default", label, className, children, ...rest },
  ref,
) {
  return (
    <FoundationTabs
      {...rest}
      ref={ref}
      label={label}
      data-slot="tabs-list"
      data-variant={variant}
      className={cn(tabsListVariants(variant), className)}
    >
      {children}
    </FoundationTabs>
  );
});

export interface TabsTriggerProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "value"> {
  value: string;
  asChild?: boolean;
}

const triggerState = (
  selected: boolean,
): {
  role: "tab";
  "aria-selected": boolean;
  "data-state": "active" | "inactive";
  "data-active": "" | undefined;
  tabIndex: number;
} => ({
  role: "tab",
  "aria-selected": selected,
  "data-state": selected ? "active" : "inactive",
  "data-active": selected ? "" : undefined,
  tabIndex: selected ? 0 : -1,
});

export const TabsTrigger = forwardRef<HTMLButtonElement, TabsTriggerProps>(function TabsTrigger(
  { value, asChild, className, children, onClick, ...rest },
  ref,
) {
  const state = useTabsAdapterState();
  const selected = state?.value === value;
  const handleClick = (event: MouseEvent<HTMLElement>): void => {
    onClick?.(event as MouseEvent<HTMLButtonElement>);
    if (!event.defaultPrevented) {
      state?.setValue(value);
    }
  };
  if (asChild && isValidElement(children)) {
    const child = children as ReactElement<ElementProps>;
    const childProps = child.props as ElementProps;
    return cloneElement(child, {
      ...triggerState(selected ?? false),
      className: cn([childProps.className, className]) || undefined,
      onClick: (event: MouseEvent<HTMLElement>) => {
        (childProps.onClick as ((event: MouseEvent<HTMLElement>) => void) | undefined)?.(event);
        handleClick(event);
      },
      ref: mergeRefs(ref as Ref<unknown>, childRefOf(child)),
    });
  }
  return (
    <Tab
      {...rest}
      ref={ref}
      selected={selected ?? false}
      data-state={selected ? "active" : "inactive"}
      data-active={selected ? "" : undefined}
      onClick={handleClick}
      className={className}
    >
      {children}
    </Tab>
  );
});

export interface TabsContentProps extends HTMLAttributes<HTMLDivElement> {
  value: string;
  forceMount?: boolean;
}

export const TabsContent = forwardRef<HTMLDivElement, TabsContentProps>(function TabsContent(
  { value, forceMount, className, children, ...rest },
  ref,
) {
  const state = useTabsAdapterState();
  const selected = state?.value === value;
  if (!selected && !forceMount) {
    return null;
  }
  return (
    <div
      {...rest}
      ref={ref}
      role="tabpanel"
      data-slot="tabs-content"
      data-state={selected ? "active" : "inactive"}
      className={cn("flex-1 text-ui-base outline-none", className)}
    >
      {children}
    </div>
  );
});
