// M4 adapter：Select（旧 API：Select/SelectTrigger/SelectValue/SelectContent/SelectItem/SelectGroup/SelectLabel）。
// 根状态、列表定位、键盘导航都在 adapter 内实现；内容常驻 DOM（关闭时 hidden）以便 SelectValue 取到选项文案。

import {
  forwardRef,
  useCallback,
  useEffect,
  useLayoutEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type ButtonHTMLAttributes,
  type CSSProperties,
  type HTMLAttributes,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";

import { Separator } from "../Separator.js";
import {
  SelectContent as FoundationSelectContent,
  SelectGroup as FoundationSelectGroup,
  SelectGroupLabel,
  SelectItem as FoundationSelectItem,
  SelectValue as FoundationSelectValue,
} from "../Select.js";
import { SelectTrigger as FoundationSelectTrigger } from "../SelectTrigger.js";
import { mergeRefs } from "./domTargets.js";
import { computePopoverPoint, type PopoverAlign, type PopoverSide } from "./popoverPosition.js";
import { SelectAdapterContext, useSelectAdapterState } from "./selectContext.js";
import { cn } from "@/components/lib/utils.js";

const useIsomorphicLayoutEffect = typeof window === "undefined" ? useEffect : useLayoutEffect;

const SELECT_TRIGGER_VARIANT_CLASS: Record<string, string> = {
  input: "",
  default: "border-transparent bg-primary text-primary-foreground hover:bg-primary/80",
  outline: "border-border bg-transparent hover:bg-input/50",
  secondary: "border-transparent bg-secondary hover:bg-secondary/80",
  ghost: "border-transparent bg-transparent hover:bg-hover",
  destructive: "border-transparent bg-destructive text-destructive-foreground",
};

function optionNodes(container: HTMLElement | null): HTMLElement[] {
  return container
    ? [...container.querySelectorAll<HTMLElement>('[role="option"]:not([disabled])')]
    : [];
}

function moveFocus(container: HTMLElement | null, delta: 1 | -1): void {
  const options = optionNodes(container);
  if (options.length === 0) {
    return;
  }
  const current = options.indexOf(document.activeElement as HTMLElement);
  const next =
    delta === 1
      ? (current + 1 + options.length) % options.length
      : (current - 1 + options.length) % options.length;
  options[next]?.focus();
}

export interface SelectProps {
  value?: string;
  defaultValue?: string;
  onValueChange?: (value: string) => void;
  open?: boolean;
  defaultOpen?: boolean;
  onOpenChange?: (open: boolean) => void;
  disabled?: boolean;
  required?: boolean;
  name?: string;
  children?: ReactNode;
}

export function Select({
  value,
  defaultValue,
  onValueChange,
  open: openProp,
  defaultOpen,
  onOpenChange,
  disabled = false,
  required: _required,
  name: _name,
  children,
}: SelectProps): ReactNode {
  const [uncontrolledValue, setUncontrolledValue] = useState(defaultValue);
  const [uncontrolledOpen, setUncontrolledOpen] = useState(defaultOpen ?? false);
  const [labels, setLabels] = useState<Record<string, string>>({});
  const triggerRef = useRef<HTMLElement | null>(null);
  const contentId = useId();
  const currentValue = value ?? uncontrolledValue;
  const open = openProp ?? uncontrolledOpen;

  const setOpen = useCallback(
    (next: boolean) => {
      if (openProp === undefined) {
        setUncontrolledOpen(next);
      }
      onOpenChange?.(next);
    },
    [openProp, onOpenChange],
  );

  const selectValue = useCallback(
    (next: string) => {
      if (value === undefined) {
        setUncontrolledValue(next);
      }
      onValueChange?.(next);
      setOpen(false);
      triggerRef.current?.focus();
    },
    [value, onValueChange, setOpen],
  );

  const registerLabel = useCallback((optionValue: string, label: string) => {
    setLabels((previous) =>
      previous[optionValue] === label ? previous : { ...previous, [optionValue]: label },
    );
    return () => {
      setLabels((previous) => {
        if (!(optionValue in previous)) {
          return previous;
        }
        const next = { ...previous };
        delete next[optionValue];
        return next;
      });
    };
  }, []);

  const state = useMemo(
    () => ({
      value: currentValue,
      open,
      setOpen,
      selectValue,
      triggerRef,
      contentId,
      labels: new Map(Object.entries(labels)),
      registerLabel,
      labelFor: (optionValue: string | undefined) =>
        optionValue === undefined ? undefined : labels[optionValue],
      disabled,
    }),
    [currentValue, open, setOpen, selectValue, contentId, labels, registerLabel, disabled],
  );

  return <SelectAdapterContext.Provider value={state}>{children}</SelectAdapterContext.Provider>;
}

export interface SelectTriggerProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  size?: "sm" | "default" | "lg" | "xs";
  variant?: "input" | "default" | "outline" | "secondary" | "ghost" | "destructive";
  indicator?: ReactNode;
}

export const SelectTrigger = forwardRef<HTMLButtonElement, SelectTriggerProps>(
  function SelectTrigger(
    { size = "default", variant = "input", indicator, className, onClick, onKeyDown, ...rest },
    ref,
  ) {
    const state = useSelectAdapterState();
    const fieldSize = size === "xs" ? "sm" : size;
    const variantClass = SELECT_TRIGGER_VARIANT_CLASS[variant] ?? "";
    const mergedClassName = cn([className, variantClass]);
    return (
      <FoundationSelectTrigger
        {...rest}
        ref={mergeRefs(ref, state?.triggerRef)}
        fieldSize={fieldSize}
        indicator={indicator}
        data-variant={variant}
        data-size={size}
        className={mergedClassName}
        open={state?.open ?? false}
        disabled={rest.disabled ?? state?.disabled}
        aria-controls={state?.contentId}
        onClick={(event) => {
          onClick?.(event);
          if (!event.defaultPrevented) {
            state?.setOpen(!state?.open);
          }
        }}
        onKeyDown={(event) => {
          onKeyDown?.(event);
          if (event.defaultPrevented) {
            return;
          }
          if (event.key === "ArrowDown" || event.key === "Enter" || event.key === " ") {
            event.preventDefault();
            state?.setOpen(true);
          }
        }}
      />
    );
  },
);

export interface SelectValueProps extends HTMLAttributes<HTMLSpanElement> {
  placeholder?: ReactNode;
}

export const SelectValue = forwardRef<HTMLSpanElement, SelectValueProps>(function SelectValue(
  { placeholder, className, children, ...rest },
  ref,
) {
  const state = useSelectAdapterState();
  const label = state?.labelFor(state.value);
  const content = label ?? children;
  return (
    <FoundationSelectValue
      {...rest}
      ref={ref}
      placeholder={content === undefined && placeholder !== undefined}
      className={className}
    >
      {content === undefined ? placeholder : content}
    </FoundationSelectValue>
  );
});

export interface SelectContentProps extends HTMLAttributes<HTMLDivElement> {
  position?: "item-aligned" | "popper";
  align?: PopoverAlign;
  side?: PopoverSide;
  sideOffset?: number;
  alignOffset?: number;
  collisionPadding?: number;
  onOpenAutoFocus?: (event: { preventDefault: () => void }) => void;
  onCloseAutoFocus?: (event: { preventDefault: () => void }) => void;
}

export const SelectContent = forwardRef<HTMLDivElement, SelectContentProps>(function SelectContent(
  {
    position: _position,
    align = "start",
    side = "bottom",
    sideOffset = 4,
    alignOffset = 0,
    collisionPadding = 8,
    className,
    children,
    style,
    ...rest
  },
  ref,
) {
  const state = useSelectAdapterState();
  const open = state?.open ?? false;
  const contentRef = useRef<HTMLDivElement | null>(null);
  const [point, setPoint] = useState<{ top: number; left: number; triggerWidth: number } | null>(
    null,
  );

  useIsomorphicLayoutEffect(() => {
    if (!open) {
      setPoint(null);
      return;
    }
    const trigger = state?.triggerRef.current;
    const content = contentRef.current;
    if (!trigger || !content || typeof window === "undefined") {
      return;
    }
    const measure = (): void => {
      const rect = content.getBoundingClientRect();
      const next = computePopoverPoint({
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
      setPoint({ top: next.top, left: next.left, triggerWidth: next.triggerWidth });
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
    const handlePointer = (event: globalThis.MouseEvent): void => {
      const target = event.target as Node | null;
      if (
        target &&
        (contentRef.current?.contains(target) || state?.triggerRef.current?.contains(target))
      ) {
        return;
      }
      state?.setOpen(false);
    };
    document.addEventListener("mousedown", handlePointer);
    return () => document.removeEventListener("mousedown", handlePointer);
  }, [open, state]);

  useEffect(() => {
    if (!open) {
      return;
    }
    const selected = contentRef.current?.querySelector<HTMLElement>(
      '[role="option"][aria-selected="true"]:not([disabled])',
    );
    const first = selected ?? optionNodes(contentRef.current)[0];
    first?.focus();
  }, [open]);

  const contentStyle: CSSProperties = {
    ...style,
    position: "fixed",
    top: point?.top ?? 0,
    left: point?.left ?? 0,
    minWidth: point?.triggerWidth,
    visibility: point ? undefined : "hidden",
  };

  const content = (
    <FoundationSelectContent
      {...rest}
      ref={mergeRefs(ref, contentRef)}
      id={state?.contentId}
      hidden={!open}
      data-state={open ? "open" : "closed"}
      style={contentStyle}
      className={className}
      onKeyDown={(event: ReactKeyboardEvent<HTMLDivElement>) => {
        rest.onKeyDown?.(event);
        if (event.defaultPrevented) {
          return;
        }
        if (event.key === "ArrowDown" || event.key === "ArrowUp") {
          event.preventDefault();
          moveFocus(contentRef.current, event.key === "ArrowDown" ? 1 : -1);
        } else if (event.key === "Escape") {
          event.preventDefault();
          state?.setOpen(false);
          state?.triggerRef.current?.focus();
        }
      }}
    >
      {children}
    </FoundationSelectContent>
  );

  return typeof document === "undefined" || !document.body
    ? content
    : createPortal(content, document.body);
});

export interface SelectItemProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "value"> {
  value: string;
  textValue?: string;
}

export const SelectItem = forwardRef<HTMLButtonElement, SelectItemProps>(function SelectItem(
  { value, textValue, onClick, children, ...rest },
  ref,
) {
  const state = useSelectAdapterState();
  const itemRef = useRef<HTMLButtonElement | null>(null);
  const registerLabel = state?.registerLabel;

  useEffect(() => {
    const label = textValue ?? itemRef.current?.textContent?.trim() ?? value;
    if (registerLabel) {
      return registerLabel(value, label);
    }
    return undefined;
  }, [value, textValue, registerLabel]);

  return (
    <FoundationSelectItem
      {...rest}
      ref={mergeRefs(ref, itemRef)}
      selected={state?.value === value}
      onClick={(event) => {
        onClick?.(event);
        if (!event.defaultPrevented) {
          state?.selectValue(value);
        }
      }}
    >
      {children}
    </FoundationSelectItem>
  );
});

export interface SelectGroupProps extends HTMLAttributes<HTMLDivElement> {
  label?: string;
}

export const SelectGroup = forwardRef<HTMLDivElement, SelectGroupProps>(function SelectGroup(
  { label, className, ...rest },
  ref,
) {
  return <FoundationSelectGroup {...rest} ref={ref} label={label} className={className} />;
});

export const SelectLabel = forwardRef<HTMLDivElement, HTMLAttributes<HTMLDivElement>>(
  function SelectLabel({ className, children, ...rest }, _ref) {
    return (
      <SelectGroupLabel {...rest} className={className}>
        {children}
      </SelectGroupLabel>
    );
  },
);

export const SelectSeparator = forwardRef<HTMLDivElement, HTMLAttributes<HTMLDivElement>>(
  function SelectSeparator({ className, ...rest }, ref) {
    return <Separator {...rest} ref={ref} className={className} />;
  },
);
