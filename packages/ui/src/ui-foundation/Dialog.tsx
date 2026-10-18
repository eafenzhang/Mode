// P1.1 Dialog shell（spec: docs/specs/m4-ui-foundation.md §2.4）。
// 只负责 shell、焦点进入/恢复、Esc 关闭与 Tab 循环；遮罩/定位由调用方组合。

import {
  forwardRef,
  useEffect,
  useRef,
  type HTMLAttributes,
  type MutableRefObject,
  type ReactNode,
} from "react";

import { dialogClassName } from "./surfaceStyles.js";

const FOCUSABLE_SELECTOR = [
  "a[href]",
  "button:not([disabled])",
  "textarea:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  '[tabindex]:not([tabindex="-1"])',
].join(",");

export interface DialogProps extends HTMLAttributes<HTMLDivElement> {
  open: boolean;
  onClose: () => void;
  /** 标题元素 id（aria-labelledby）。 */
  labelledBy?: string;
  children?: ReactNode;
}

export const Dialog = forwardRef<HTMLDivElement, DialogProps>(function Dialog(
  { open, onClose, labelledBy, className, children, ...rest },
  ref,
) {
  const nodeRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!open) {
      return;
    }
    const node = nodeRef.current;
    if (!node) {
      return;
    }
    const previous = document.activeElement as HTMLElement | null;
    const focusable = (): HTMLElement[] =>
      [...node.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)].filter(
        (element) => element.getClientRects().length > 0,
      );
    (focusable()[0] ?? node).focus();

    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key !== "Tab") {
        return;
      }
      const items = focusable();
      if (items.length === 0) {
        event.preventDefault();
        node.focus();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      if (!first || !last) {
        event.preventDefault();
        node.focus();
        return;
      }
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };

    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      previous?.focus?.();
    };
  }, [open, onClose]);

  if (!open) {
    return null;
  }

  const assignRef = (node: HTMLDivElement | null): void => {
    nodeRef.current = node;
    if (typeof ref === "function") {
      ref(node);
    } else if (ref) {
      (ref as MutableRefObject<HTMLDivElement | null>).current = node;
    }
  };

  return (
    <div
      {...rest}
      ref={assignRef}
      role="dialog"
      aria-modal="true"
      aria-labelledby={labelledBy}
      tabIndex={-1}
      className={dialogClassName(className)}
    >
      {children}
    </div>
  );
});
