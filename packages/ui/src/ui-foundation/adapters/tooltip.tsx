// M4 adapter：Tooltip（旧 API：Tooltip/TooltipTrigger/TooltipContent/TooltipProvider）。
// 显示策略：悬停/聚焦按 delayDuration 打开，离开/失焦关闭，Escape 关闭。
// 内容沿用 foundation 的 tooltip 视觉契约（pointer-events-none）。

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
  type CSSProperties,
  type HTMLAttributes,
  type FocusEvent as ReactFocusEvent,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactElement,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";

import { Tooltip as FoundationTooltip } from "../Tooltip.js";
import { childRefOf, mergeRefs } from "./domTargets.js";
import { computePopoverPoint, type PopoverAlign, type PopoverSide } from "./popoverPosition.js";
import {
  TooltipAdapterContext,
  TooltipProviderContext,
  useTooltipAdapterState,
  useTooltipProviderState,
  type TooltipProviderState,
} from "./tooltipContext.js";

type ElementProps = Record<string, unknown>;

const useIsomorphicLayoutEffect = typeof window === "undefined" ? useEffect : useLayoutEffect;

export interface TooltipProviderProps {
  delayDuration?: number;
  skipDelayDuration?: number;
  disableHoverableContent?: boolean;
  children?: ReactNode;
}

export function TooltipProvider({
  delayDuration = 0,
  skipDelayDuration = 300,
  disableHoverableContent: _disableHoverableContent,
  children,
}: TooltipProviderProps): ReactNode {
  const value = useMemo<TooltipProviderState>(
    () => ({ delayDuration, skipDelayDuration }),
    [delayDuration, skipDelayDuration],
  );
  return (
    <TooltipProviderContext.Provider value={value}>{children}</TooltipProviderContext.Provider>
  );
}

export interface TooltipProps {
  open?: boolean;
  defaultOpen?: boolean;
  onOpenChange?: (open: boolean) => void;
  delayDuration?: number;
  disableHoverableContent?: boolean;
  children?: ReactNode;
}

export function Tooltip({
  open,
  defaultOpen,
  onOpenChange,
  delayDuration,
  disableHoverableContent: _disableHoverableContent,
  children,
}: TooltipProps): ReactNode {
  const provider = useTooltipProviderState();
  const [uncontrolled, setUncontrolled] = useState(defaultOpen ?? false);
  const current = open ?? uncontrolled;
  const triggerRef = useRef<HTMLElement | null>(null);
  const contentId = useId();
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const delay = delayDuration ?? provider?.delayDuration ?? 0;

  const cancelPending = useCallback(() => {
    if (timerRef.current !== null) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
  }, []);

  const setOpen = useCallback(
    (next: boolean) => {
      if (open === undefined) {
        setUncontrolled(next);
      }
      onOpenChange?.(next);
    },
    [open, onOpenChange],
  );

  const scheduleOpen = useCallback(
    (delayMs?: number) => {
      cancelPending();
      const wait = delayMs ?? delay;
      if (wait <= 0) {
        setOpen(true);
        return;
      }
      timerRef.current = setTimeout(() => {
        timerRef.current = null;
        setOpen(true);
      }, wait);
    },
    [cancelPending, delay, setOpen],
  );

  useEffect(() => cancelPending, [cancelPending]);

  const context = useMemo(
    () => ({
      open: current,
      setOpen,
      scheduleOpen,
      cancelPending,
      triggerRef,
      contentId,
      delayDuration: delay,
    }),
    [current, setOpen, scheduleOpen, cancelPending, contentId, delay],
  );
  return (
    <TooltipAdapterContext.Provider value={context}>{children}</TooltipAdapterContext.Provider>
  );
}

export interface TooltipTriggerProps extends HTMLAttributes<HTMLElement> {
  asChild?: boolean;
}

export const TooltipTrigger = forwardRef<HTMLElement, TooltipTriggerProps>(function TooltipTrigger(
  { asChild, children, onPointerEnter, onPointerLeave, onFocus, onBlur, ...rest },
  ref,
) {
  const state = useTooltipAdapterState();
  const handlers = {
    onPointerEnter: (event: ReactPointerEvent<HTMLElement>) => {
      onPointerEnter?.(event);
      if (!event.defaultPrevented) {
        state?.scheduleOpen();
      }
    },
    onPointerLeave: (event: ReactPointerEvent<HTMLElement>) => {
      onPointerLeave?.(event);
      state?.cancelPending();
      state?.setOpen(false);
    },
    onFocus: (event: ReactFocusEvent<HTMLElement>) => {
      onFocus?.(event);
      state?.scheduleOpen(0);
    },
    onBlur: (event: ReactFocusEvent<HTMLElement>) => {
      onBlur?.(event);
      state?.cancelPending();
      state?.setOpen(false);
    },
    "aria-describedby": state?.open ? state.contentId : undefined,
    "data-state": state?.open ? "open" : "closed",
    "data-slot": "tooltip-trigger",
  };
  if (asChild && isValidElement(children)) {
    const child = children as ReactElement<ElementProps>;
    const childProps = child.props;
    return cloneElement(child, {
      ...handlers,
      onPointerEnter: (event: ReactPointerEvent<HTMLElement>) => {
        (
          childProps.onPointerEnter as ((event: ReactMouseEvent<HTMLElement>) => void) | undefined
        )?.(event);
        handlers.onPointerEnter(event);
      },
      onPointerLeave: (event: ReactPointerEvent<HTMLElement>) => {
        (
          childProps.onPointerLeave as ((event: ReactMouseEvent<HTMLElement>) => void) | undefined
        )?.(event);
        handlers.onPointerLeave(event);
      },
      onFocus: (event: ReactFocusEvent<HTMLElement>) => {
        (childProps.onFocus as ((event: ReactFocusEvent<HTMLElement>) => void) | undefined)?.(
          event,
        );
        handlers.onFocus(event);
      },
      onBlur: (event: ReactFocusEvent<HTMLElement>) => {
        (childProps.onBlur as ((event: ReactFocusEvent<HTMLElement>) => void) | undefined)?.(event);
        handlers.onBlur(event);
      },
      ref: mergeRefs(ref, state?.triggerRef, childRefOf<HTMLElement>(child)),
    });
  }
  return (
    <button {...rest} {...handlers} ref={mergeRefs(ref, state?.triggerRef)} type="button">
      {children}
    </button>
  );
});

export interface TooltipContentProps extends HTMLAttributes<HTMLDivElement> {
  side?: PopoverSide;
  align?: PopoverAlign;
  sideOffset?: number;
  alignOffset?: number;
  collisionPadding?: number;
}

export const TooltipContent = forwardRef<HTMLDivElement, TooltipContentProps>(
  function TooltipContent(
    {
      side = "top",
      align = "center",
      sideOffset = 0,
      alignOffset = 0,
      collisionPadding = 4,
      className,
      children,
      ...rest
    },
    ref,
  ) {
    const state = useTooltipAdapterState();
    const open = state?.open ?? false;
    const contentRef = useRef<HTMLDivElement | null>(null);
    const [position, setPosition] = useState<{ top: number; left: number } | null>(null);

    useIsomorphicLayoutEffect(() => {
      if (!open) {
        setPosition(null);
        return;
      }
      const trigger = state?.triggerRef.current;
      const content = contentRef.current;
      if (!trigger || !content || typeof window === "undefined") {
        return;
      }
      const measure = (): void => {
        const rect = content.getBoundingClientRect();
        const point = computePopoverPoint({
          anchor: trigger.getBoundingClientRect(),
          content: { width: rect.width, height: rect.height },
          align,
          side,
          sideOffset,
          alignOffset,
          avoidCollisions: true,
          collisionPadding,
          viewport: { width: window.innerWidth, height: window.innerHeight },
        });
        setPosition({ top: point.top, left: point.left });
      };
      measure();
      window.addEventListener("resize", measure);
      window.addEventListener("scroll", measure, true);
      return () => {
        window.removeEventListener("resize", measure);
        window.removeEventListener("scroll", measure, true);
      };
    }, [open, state, align, side, sideOffset, alignOffset, collisionPadding]);

    useEffect(() => {
      if (!open || typeof document === "undefined") {
        return;
      }
      const handleKey = (event: KeyboardEvent): void => {
        if (event.key === "Escape") {
          state?.setOpen(false);
        }
      };
      document.addEventListener("keydown", handleKey);
      return () => document.removeEventListener("keydown", handleKey);
    }, [open, state]);

    if (!open) {
      return null;
    }

    const style: CSSProperties = {
      ...rest.style,
      position: "fixed",
      top: position?.top ?? 0,
      left: position?.left ?? 0,
      visibility: position ? undefined : "hidden",
    };

    const content = (
      <FoundationTooltip
        {...rest}
        ref={mergeRefs(ref, contentRef)}
        id={state?.contentId}
        data-slot="tooltip-content"
        data-side={side}
        data-align={align}
        data-state="open"
        style={style}
        className={className}
      >
        {children}
      </FoundationTooltip>
    );

    return typeof document === "undefined" || !document.body
      ? content
      : createPortal(content, document.body);
  },
);
