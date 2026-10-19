// M4 adapter：Dialog（旧 API：Dialog open/onOpenChange + Content/Trigger/Close 组合）。
// Content 仍由 foundation Dialog 负责 shell 与焦点管理；adapter 只补旧的状态组合语义。

import {
  cloneElement,
  forwardRef,
  useCallback,
  useEffect,
  useMemo,
  useState,
  type HTMLAttributes,
  type MouseEvent as ReactMouseEvent,
  type ReactElement,
  type ReactNode,
} from "react";

import {
  Dialog as FoundationDialog,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "../Dialog.js";
import { DialogAdapterContext, useDialogAdapterState } from "./dialogContext.js";
import { cn } from "@/components/lib/utils.js";

export { DialogDescription, DialogFooter, DialogHeader, DialogTitle };

type ClickableElement = ReactElement<{ onClick?: (event: ReactMouseEvent) => void }>;

function callChildClick(children: ClickableElement, event: ReactMouseEvent): boolean {
  children.props.onClick?.(event);
  return event.defaultPrevented;
}

export interface DialogProps {
  open?: boolean;
  defaultOpen?: boolean;
  onOpenChange?: (open: boolean) => void;
  modal?: boolean;
  children?: ReactNode;
}

export function Dialog({
  open,
  defaultOpen = false,
  onOpenChange,
  modal: _modal,
  children,
}: DialogProps): ReactNode {
  const [uncontrolledOpen, setUncontrolledOpen] = useState(defaultOpen);
  const controlled = open !== undefined;
  const current = controlled ? open : uncontrolledOpen;
  const setOpen = useCallback(
    (next: boolean) => {
      if (!controlled) {
        setUncontrolledOpen(next);
      }
      onOpenChange?.(next);
    },
    [controlled, onOpenChange],
  );
  const value = useMemo(() => ({ open: current, setOpen }), [current, setOpen]);
  return <DialogAdapterContext.Provider value={value}>{children}</DialogAdapterContext.Provider>;
}

export interface DialogContentProps extends HTMLAttributes<HTMLDivElement> {
  labelledBy?: string;
  showCloseButton?: boolean;
  showOverlay?: boolean;
  overlayClassName?: string;
  onEscapeKeyDown?: (event: Event) => void;
  onInteractOutside?: (event: { target: EventTarget | null; preventDefault: () => void }) => void;
  onOpenAutoFocus?: (event: { preventDefault: () => void }) => void;
  onCloseAutoFocus?: (event: { preventDefault: () => void }) => void;
}

export const DialogContent = forwardRef<HTMLDivElement, DialogContentProps>(function DialogContent(
  {
    className,
    children,
    labelledBy,
    showCloseButton = true,
    showOverlay = true,
    overlayClassName,
    onEscapeKeyDown,
    onInteractOutside,
    onOpenAutoFocus,
    onCloseAutoFocus: _onCloseAutoFocus,
    ...rest
  },
  ref,
) {
  const state = useDialogAdapterState();
  const open = state?.open ?? false;

  useEffect(() => {
    if (!open || typeof document === "undefined" || !onEscapeKeyDown) {
      return;
    }
    const handleKey = (event: KeyboardEvent): void => {
      if (event.key !== "Escape") {
        return;
      }
      onEscapeKeyDown(event);
      if (event.defaultPrevented) {
        event.stopPropagation();
      }
    };
    document.addEventListener("keydown", handleKey, true);
    return () => document.removeEventListener("keydown", handleKey, true);
  }, [open, onEscapeKeyDown]);

  if (!open || !state) {
    return null;
  }

  const handleOverlayClick = (event: ReactMouseEvent<HTMLDivElement>): void => {
    if (event.target !== event.currentTarget) {
      return;
    }
    const outside = {
      defaultPrevented: false,
      preventDefault: () => (outside.defaultPrevented = true),
    };
    onInteractOutside?.({ target: event.target, preventDefault: outside.preventDefault });
    if (!outside.defaultPrevented) {
      state.setOpen(false);
    }
  };

  const hasWidth = /(^|\s)w-/.test(className ?? "");
  const hasMaxWidth = /(^|\s)max-w-/.test(className ?? "");
  const mergedPanelClassName = cn([
    className,
    "relative",
    hasWidth ? "" : "w-full",
    hasMaxWidth ? "" : "max-w-lg",
  ]);
  return (
    <div
      data-slot="dialog-overlay"
      onClick={showOverlay ? handleOverlayClick : undefined}
      className={cn([
        "fixed inset-0 z-50 flex items-center justify-center overflow-y-auto p-4",
        showOverlay ? "bg-black/50" : "",
        overlayClassName,
      ])}
    >
      <FoundationDialog
        {...rest}
        ref={ref}
        open
        onClose={() => state.setOpen(false)}
        labelledBy={labelledBy}
        data-slot="dialog-content"
        className={mergedPanelClassName}
      >
        {children}
        {showCloseButton ? (
          <button
            type="button"
            aria-label="Close"
            onClick={() => state.setOpen(false)}
            className="absolute end-3 top-3 inline-flex size-6 items-center justify-center rounded-sm text-foreground-subtle hover:bg-hover"
          >
            <svg
              aria-hidden="true"
              viewBox="0 0 16 16"
              className="size-4"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.5"
              strokeLinecap="round"
            >
              <path d="M4 4l8 8M12 4l-8 8" />
            </svg>
          </button>
        ) : null}
      </FoundationDialog>
    </div>
  );
});

export interface DialogTriggerProps {
  children: ClickableElement;
  asChild?: boolean;
}

export function DialogTrigger({ children }: DialogTriggerProps): ReactElement {
  const state = useDialogAdapterState();
  return cloneElement(children, {
    onClick: (event: ReactMouseEvent) => {
      if (!callChildClick(children, event)) {
        state?.setOpen(true);
      }
    },
  });
}

export interface DialogCloseProps {
  children: ClickableElement;
  asChild?: boolean;
}

export function DialogClose({ children }: DialogCloseProps): ReactElement {
  const state = useDialogAdapterState();
  return cloneElement(children, {
    onClick: (event: ReactMouseEvent) => {
      if (!callChildClick(children, event)) {
        state?.setOpen(false);
      }
    },
  });
}

/** 兼容旧导出名：foundation 由 Host/DOM 组合，不做 portal 分发。 */
export function DialogPortal({ children }: { children?: ReactNode }): ReactNode {
  return children;
}
