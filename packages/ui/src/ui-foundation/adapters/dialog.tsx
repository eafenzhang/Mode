// M4 adapter：Dialog（旧 API：Dialog open/onOpenChange + Content/Trigger/Close 组合）。
// Content 仍由 foundation Dialog 负责 shell 与焦点管理；adapter 只补旧的状态组合语义。

import {
  cloneElement,
  forwardRef,
  useCallback,
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
  children?: ReactNode;
}

export function Dialog({
  open,
  defaultOpen = false,
  onOpenChange,
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
}

export const DialogContent = forwardRef<HTMLDivElement, DialogContentProps>(function DialogContent(
  { className, children, labelledBy, ...rest },
  ref,
) {
  const state = useDialogAdapterState();
  if (!state?.open) {
    return null;
  }
  return (
    <FoundationDialog
      {...rest}
      ref={ref}
      open
      onClose={() => state.setOpen(false)}
      labelledBy={labelledBy}
      className={className}
    >
      {children}
    </FoundationDialog>
  );
});

export interface DialogTriggerProps {
  children: ClickableElement;
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
