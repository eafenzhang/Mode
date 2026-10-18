// P1.1 Collapsible primitive（spec: docs/specs/m4-ui-foundation.md §2.7）。
// 只负责展开状态、data-state 与触发器的可访问性；视觉由调用方 className 决定。

import {
  cloneElement,
  createContext,
  forwardRef,
  isValidElement,
  useCallback,
  useContext,
  useMemo,
  useState,
  type ButtonHTMLAttributes,
  type HTMLAttributes,
  type MouseEvent as ReactMouseEvent,
  type ReactElement,
  type ReactNode,
} from "react";

export interface CollapsibleProps extends HTMLAttributes<HTMLDivElement> {
  open?: boolean;
  defaultOpen?: boolean;
  onOpenChange?: (open: boolean) => void;
  asChild?: boolean;
}

interface CollapsibleContextValue {
  open: boolean;
  setOpen: (open: boolean) => void;
}

const CollapsibleContext = createContext<CollapsibleContextValue | null>(null);

function useCollapsible(): CollapsibleContextValue | null {
  return useContext(CollapsibleContext);
}

export function Collapsible({
  open,
  defaultOpen = false,
  onOpenChange,
  asChild,
  className,
  children,
  ...rest
}: CollapsibleProps): ReactNode {
  const [uncontrolled, setUncontrolled] = useState(defaultOpen);
  const current = open ?? uncontrolled;
  const setOpen = useCallback(
    (next: boolean) => {
      if (open === undefined) {
        setUncontrolled(next);
      }
      onOpenChange?.(next);
    },
    [open, onOpenChange],
  );
  const value = useMemo(() => ({ open: current, setOpen }), [current, setOpen]);
  const state = current ? "open" : "closed";
  if (asChild && isValidElement(children)) {
    const child = children as ReactElement<Record<string, unknown>>;
    return (
      <CollapsibleContext.Provider value={value}>
        {cloneElement(child, { "data-state": state })}
      </CollapsibleContext.Provider>
    );
  }
  return (
    <CollapsibleContext.Provider value={value}>
      <div {...rest} data-state={state} className={className}>
        {children}
      </div>
    </CollapsibleContext.Provider>
  );
}

export interface CollapsibleTriggerProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  asChild?: boolean;
}

export const CollapsibleTrigger = forwardRef<HTMLButtonElement, CollapsibleTriggerProps>(
  function CollapsibleTrigger({ asChild, className, children, onClick, ...rest }, ref) {
    const context = useCollapsible();
    const open = context?.open ?? false;
    const handleClick = (event: ReactMouseEvent<HTMLElement>): void => {
      onClick?.(event as ReactMouseEvent<HTMLButtonElement>);
      if (!event.defaultPrevented) {
        context?.setOpen(!open);
      }
    };
    const triggerProps = {
      "aria-expanded": open,
      "data-state": open ? "open" : "closed",
      "data-slot": "collapsible-trigger",
      onClick: handleClick,
    };
    if (asChild && isValidElement(children)) {
      const child = children as ReactElement<Record<string, unknown>>;
      const childProps = child.props;
      return cloneElement(child, {
        ...triggerProps,
        onClick: (event: ReactMouseEvent<HTMLElement>) => {
          (childProps.onClick as ((event: ReactMouseEvent<HTMLElement>) => void) | undefined)?.(
            event,
          );
          handleClick(event);
        },
      });
    }
    return (
      <button {...rest} {...triggerProps} ref={ref} type="button" className={className}>
        {children}
      </button>
    );
  },
);

export interface CollapsibleContentProps extends HTMLAttributes<HTMLDivElement> {
  forceMount?: boolean;
  asChild?: boolean;
}

export const CollapsibleContent = forwardRef<HTMLDivElement, CollapsibleContentProps>(
  function CollapsibleContent({ forceMount, asChild, children, ...rest }, ref) {
    const context = useCollapsible();
    const open = context?.open ?? false;
    if (!open && !forceMount) {
      return null;
    }
    const contentProps = {
      ...rest,
      "data-state": open ? "open" : "closed",
      "data-slot": "collapsible-content",
      ref,
    };
    if (asChild && isValidElement(children)) {
      return cloneElement(children as ReactElement<Record<string, unknown>>, contentProps);
    }
    return <div {...contentProps}>{children}</div>;
  },
);
