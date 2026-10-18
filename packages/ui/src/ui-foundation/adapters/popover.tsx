// M4 adapter：Popover（旧 API：Popover/PopoverTrigger/PopoverContent/PopoverAnchor/Header/Title/Description）。
// 定位按锚点 rect 计算 fixed 坐标；保留 Radix 暴露的 trigger-width / available-height CSS 变量。

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
  type MouseEvent as ReactMouseEvent,
  type ReactElement,
  type ReactNode,
} from "react";

import { createPortal } from "react-dom";

import { Popover as FoundationPopover } from "../Popover.js";
import { childRefOf, mergeRefs } from "./domTargets.js";
import {
  POPOVER_FOCUSABLE_SELECTOR,
  PopoverAdapterContext,
  preventableEvent,
  usePopoverAdapterState,
  type PopoverAnchorTarget,
  type PopoverEvent,
  type PopoverFocusEvent,
  type PopoverPointerEvent,
} from "./popoverContext.js";
import {
  computePopoverPoint,
  type PopoverAlign,
  type PopoverPoint,
  type PopoverSide,
} from "./popoverPosition.js";

type ElementProps = Record<string, unknown>;

const useIsomorphicLayoutEffect = typeof window === "undefined" ? useEffect : useLayoutEffect;

export interface PopoverProps {
  open?: boolean;
  defaultOpen?: boolean;
  onOpenChange?: (open: boolean) => void;
  modal?: boolean;
  children?: ReactNode;
}

export function Popover({
  open,
  defaultOpen,
  onOpenChange,
  modal: _modal,
  children,
}: PopoverProps): ReactNode {
  const [uncontrolled, setUncontrolled] = useState(defaultOpen ?? false);
  const current = open ?? uncontrolled;
  const anchorRef = useRef<PopoverAnchorTarget | null>(null);
  const contentId = useId();
  const setOpen = useCallback(
    (next: boolean) => {
      if (open === undefined) {
        setUncontrolled(next);
      }
      onOpenChange?.(next);
    },
    [open, onOpenChange],
  );
  const context = useMemo(
    () => ({ open: current, setOpen, anchorRef, contentId }),
    [current, setOpen, contentId],
  );
  return (
    <PopoverAdapterContext.Provider value={context}>{children}</PopoverAdapterContext.Provider>
  );
}

export interface PopoverTriggerProps extends HTMLAttributes<HTMLElement> {
  asChild?: boolean;
}

export const PopoverTrigger = forwardRef<HTMLElement, PopoverTriggerProps>(function PopoverTrigger(
  { asChild, children, onClick, ...rest },
  ref,
) {
  const state = usePopoverAdapterState();
  const handleClick = (event: ReactMouseEvent<HTMLElement>): void => {
    onClick?.(event);
    if (!event.defaultPrevented) {
      state?.setOpen(!state.open);
    }
  };
  const triggerProps = {
    "aria-expanded": state?.open ?? false,
    "aria-controls": state?.open ? state.contentId : undefined,
    "aria-haspopup": "dialog" as const,
    "data-state": state?.open ? "open" : "closed",
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
      ref: mergeRefs(ref, state?.anchorRef, childRefOf<HTMLElement>(child)),
    });
  }
  return (
    <button
      {...rest}
      {...triggerProps}
      ref={mergeRefs(ref, state?.anchorRef)}
      type="button"
      onClick={handleClick}
    >
      {children}
    </button>
  );
});

export interface PopoverAnchorProps extends HTMLAttributes<HTMLElement> {
  asChild?: boolean;
  virtualRef?: { current: PopoverAnchorTarget | null };
}

export const PopoverAnchor = forwardRef<HTMLElement, PopoverAnchorProps>(function PopoverAnchor(
  { asChild, virtualRef, children, ...rest },
  ref,
) {
  const state = usePopoverAdapterState();
  useEffect(() => {
    if (state && virtualRef) {
      state.anchorRef.current = virtualRef.current;
    }
  }, [state, virtualRef]);
  if (virtualRef) {
    return null;
  }
  if (asChild && isValidElement(children)) {
    const child = children as ReactElement<ElementProps>;
    return cloneElement(child, {
      ref: mergeRefs(ref, state?.anchorRef, childRefOf<HTMLElement>(child)),
    });
  }
  return (
    <div {...rest} ref={mergeRefs(ref, state?.anchorRef)} data-slot="popover-anchor">
      {children}
    </div>
  );
});

export interface PopoverContentProps extends HTMLAttributes<HTMLDivElement> {
  align?: PopoverAlign;
  side?: PopoverSide;
  sideOffset?: number;
  alignOffset?: number;
  avoidCollisions?: boolean;
  collisionPadding?: number;
  collisionBoundary?: Element | null;
  sticky?: "partial" | "always";
  hideWhenDetached?: boolean;
  updatePositionStrategy?: "always" | "optimized";
  forceMount?: boolean;
  onOpenAutoFocus?: (event: PopoverEvent) => void;
  onCloseAutoFocus?: (event: PopoverEvent) => void;
  onInteractOutside?: (event: PopoverPointerEvent) => void;
  onPointerDownOutside?: (event: PopoverPointerEvent) => void;
  onFocusOutside?: (event: PopoverEvent) => void;
  onEscapeKeyDown?: (event: PopoverFocusEvent) => void;
}

export const PopoverContent = forwardRef<HTMLDivElement, PopoverContentProps>(
  function PopoverContent(
    {
      align = "center",
      side = "bottom",
      sideOffset = 4,
      alignOffset = 0,
      avoidCollisions = true,
      collisionPadding = 0,
      collisionBoundary: _collisionBoundary,
      sticky: _sticky,
      hideWhenDetached: _hideWhenDetached,
      updatePositionStrategy: _updatePositionStrategy,
      forceMount,
      onOpenAutoFocus,
      onCloseAutoFocus,
      onInteractOutside,
      onPointerDownOutside,
      onFocusOutside,
      onEscapeKeyDown,
      className,
      children,
      ...rest
    },
    ref,
  ) {
    const state = usePopoverAdapterState();
    const open = state?.open ?? false;
    const contentRef = useRef<HTMLDivElement | null>(null);
    const [point, setPoint] = useState<PopoverPoint | null>(null);
    // 调用点普遍传内联回调；放进 ref 避免 effect 每次渲染都重新执行焦点逻辑。
    const focusCallbacks = useRef({ onOpenAutoFocus, onCloseAutoFocus });
    focusCallbacks.current = { onOpenAutoFocus, onCloseAutoFocus };

    useIsomorphicLayoutEffect(() => {
      if (!open) {
        setPoint(null);
        return;
      }
      const anchor = state?.anchorRef.current;
      const content = contentRef.current;
      if (!anchor || !content || typeof window === "undefined") {
        return;
      }
      const measure = (): void => {
        const rect = content.getBoundingClientRect();
        setPoint(
          computePopoverPoint({
            anchor: anchor.getBoundingClientRect(),
            content: { width: rect.width, height: rect.height },
            align,
            side,
            sideOffset,
            alignOffset,
            avoidCollisions,
            collisionPadding,
            viewport: { width: window.innerWidth, height: window.innerHeight },
          }),
        );
      };
      measure();
      window.addEventListener("resize", measure);
      window.addEventListener("scroll", measure, true);
      return () => {
        window.removeEventListener("resize", measure);
        window.removeEventListener("scroll", measure, true);
      };
    }, [open, state, align, side, sideOffset, alignOffset, avoidCollisions, collisionPadding]);

    useEffect(() => {
      if (!open || typeof document === "undefined") {
        return;
      }
      const isInside = (target: Node | null): boolean =>
        target !== null &&
        (contentRef.current?.contains(target) === true ||
          state?.anchorRef.current?.contains?.(target) === true);

      const handlePointer = (event: globalThis.MouseEvent): void => {
        const target = event.target as Node | null;
        if (isInside(target)) {
          return;
        }
        const outside = preventableEvent();
        const payload: PopoverPointerEvent = {
          target: event.target,
          currentTarget: contentRef.current,
          detail: { originalEvent: event },
          preventDefault: outside.preventDefault,
        };
        onInteractOutside?.(payload);
        onPointerDownOutside?.(payload);
        if (!outside.defaultPrevented) {
          state?.setOpen(false);
        }
      };
      const handleKey = (event: KeyboardEvent): void => {
        if (event.key !== "Escape") {
          return;
        }
        const escape = preventableEvent();
        onEscapeKeyDown?.({
          target: event.target,
          currentTarget: contentRef.current,
          isComposing: event.isComposing,
          preventDefault: escape.preventDefault,
        });
        if (!escape.defaultPrevented) {
          state?.setOpen(false);
        }
      };
      const handleFocusIn = (event: FocusEvent): void => {
        if (isInside(event.target as Node | null)) {
          return;
        }
        const focus = preventableEvent();
        onFocusOutside?.({
          target: event.target,
          currentTarget: contentRef.current,
          preventDefault: focus.preventDefault,
        });
        if (!focus.defaultPrevented) {
          state?.setOpen(false);
        }
      };
      document.addEventListener("mousedown", handlePointer);
      document.addEventListener("keydown", handleKey);
      document.addEventListener("focusin", handleFocusIn);
      return () => {
        document.removeEventListener("mousedown", handlePointer);
        document.removeEventListener("keydown", handleKey);
        document.removeEventListener("focusin", handleFocusIn);
      };
    }, [open, state, onInteractOutside, onPointerDownOutside, onFocusOutside, onEscapeKeyDown]);

    useEffect(() => {
      if (!open) {
        return;
      }
      const event = preventableEvent();
      focusCallbacks.current.onOpenAutoFocus?.({
        target: contentRef.current,
        currentTarget: contentRef.current,
        preventDefault: event.preventDefault,
      });
      if (!event.defaultPrevented) {
        contentRef.current?.querySelector<HTMLElement>(POPOVER_FOCUSABLE_SELECTOR)?.focus();
      }
      return () => {
        const closeEvent = preventableEvent();
        focusCallbacks.current.onCloseAutoFocus?.({
          target: contentRef.current,
          currentTarget: contentRef.current,
          preventDefault: closeEvent.preventDefault,
        });
        if (!closeEvent.defaultPrevented) {
          state?.anchorRef.current?.focus?.();
        }
      };
    }, [open, state]);

    if (!open && !forceMount) {
      return null;
    }

    const style: CSSProperties & Record<`--${string}`, string> = {
      ...rest.style,
      position: "fixed",
      top: point?.top ?? 0,
      left: point?.left ?? 0,
      visibility: point ? undefined : "hidden",
      "--radix-popover-trigger-width": `${point?.triggerWidth ?? 0}px`,
      "--radix-popover-content-available-height": `${point?.availableHeight ?? 0}px`,
    };

    const content = (
      <FoundationPopover
        {...rest}
        ref={mergeRefs(ref, contentRef)}
        id={state?.contentId}
        data-slot="popover-content"
        data-state={open ? "open" : "closed"}
        style={style}
        className={className}
      >
        {children}
      </FoundationPopover>
    );

    // 与 Radix 一致：浮层挂到 body，避免被祖先 overflow/transform 裁切；SSR 直接内联渲染。
    return typeof document === "undefined" || !document.body
      ? content
      : createPortal(content, document.body);
  },
);

export function PopoverHeader({ className, ...rest }: HTMLAttributes<HTMLDivElement>): ReactNode {
  return <div {...rest} data-slot="popover-header" className={className} />;
}

export function PopoverTitle({
  className,
  ...rest
}: HTMLAttributes<HTMLHeadingElement>): ReactNode {
  return <h2 {...rest} data-slot="popover-title" className={className} />;
}

export function PopoverDescription({
  className,
  ...rest
}: HTMLAttributes<HTMLParagraphElement>): ReactNode {
  return <p {...rest} data-slot="popover-description" className={className} />;
}
