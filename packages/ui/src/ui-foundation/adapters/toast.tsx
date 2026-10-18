// M4 adapter：toast 命令式 API 与宿主（旧 API：toast/updateToast/dismissToast）。
// 首次调用时把宿主挂到 body；队列与计时在 toastStore.ts。

import { useEffect, useSyncExternalStore, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";

import { Toast as FoundationToast } from "../Toast.js";
import {
  DEFAULT_TOAST_DURATION_MS,
  getToastItems,
  onToastHostRequest,
  patchToast,
  pushToast,
  removeToast,
  resolveToastAnchorLeft,
  resolveToastStackClassName,
  subscribeToasts,
  upsertToastItem,
  type ToastItem,
  type ToastOptions,
  type ToastPosition,
  type ToastUpdate,
} from "./toastStore.js";

export {
  DEFAULT_TOAST_DURATION_MS,
  resolveToastAnchorLeft,
  resolveToastStackClassName,
  upsertToastItem,
  type ToastItem,
  type ToastOptions,
  type ToastPosition,
  type ToastUpdate,
};

let hostRoot: Root | null = null;

function ensureHost(): void {
  if (typeof document === "undefined" || !document.body || hostRoot) {
    return;
  }
  const container = document.createElement("div");
  container.setAttribute("data-slot", "toast-host");
  document.body.appendChild(container);
  hostRoot = createRoot(container);
  hostRoot.render(<ToastViewport />);
}

onToastHostRequest(ensureHost);

function ToastRow({ item }: { item: ToastItem }): ReactNode {
  useEffect(() => {
    if (item.durationMs <= 0) {
      return;
    }
    const timer = setTimeout(() => removeToast(item.id), item.durationMs);
    return () => clearTimeout(timer);
  }, [item.id, item.durationMs]);

  return (
    <FoundationToast
      destructive={item.variant === "warning"}
      data-toast-id={item.id}
      data-variant={item.variant ?? "default"}
      className="pointer-events-auto flex max-w-[min(28rem,calc(100vw-2rem))] items-start gap-3"
    >
      <span className="min-w-0 flex-1 break-words">{item.message}</span>
      {item.actionLabel && item.onAction ? (
        <button
          type="button"
          className="shrink-0 rounded-md px-2 py-0.5 text-ui-sm font-medium text-foreground hover:bg-hover"
          onClick={() => {
            item.onAction?.();
            removeToast(item.id);
          }}
        >
          {item.actionLabel}
        </button>
      ) : null}
      {item.dismissible ? (
        <button
          type="button"
          aria-label={item.dismissLabel ?? "Dismiss"}
          className="shrink-0 rounded-md p-0.5 text-foreground-subtle hover:bg-hover hover:text-foreground"
          onClick={() => removeToast(item.id)}
        >
          <svg
            aria-hidden="true"
            viewBox="0 0 16 16"
            className="size-3.5"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
          >
            <path d="M4 4l8 8M12 4l-8 8" />
          </svg>
        </button>
      ) : null}
    </FoundationToast>
  );
}

export function ToastViewport(): ReactNode {
  const items = useSyncExternalStore(subscribeToasts, getToastItems, getToastItems);
  const positions: ToastPosition[] = ["top-right", "top-center", "bottom-left", "bottom-center"];
  return (
    <>
      {positions.map((position) => {
        const group = items.filter((item) => item.position === position);
        if (group.length === 0) {
          return null;
        }
        return (
          <div
            key={position}
            data-toast-stack={position}
            className={resolveToastStackClassName(position)}
          >
            {group.map((item) => (
              <ToastRow key={item.id} item={item} />
            ))}
          </div>
        );
      })}
    </>
  );
}

/** 兼容旧导出：渲染单条 toast 内容（不含宿主定位）。 */
export function ToastMessageView({
  message,
  variant,
  actionLabel,
  onAction,
  dismissible,
  onDismiss,
}: {
  message: string;
  variant?: ToastItem["variant"];
  actionLabel?: string;
  onAction?: () => void;
  dismissible?: boolean;
  onDismiss?: () => void;
}): ReactNode {
  return (
    <FoundationToast destructive={variant === "warning"} data-variant={variant ?? "default"}>
      <span className="min-w-0 flex-1 break-words">{message}</span>
      {actionLabel && onAction ? (
        <button
          type="button"
          className="shrink-0 rounded-md px-2 py-0.5 text-ui-sm font-medium"
          onClick={onAction}
        >
          {actionLabel}
        </button>
      ) : null}
      {dismissible ? (
        <button type="button" aria-label="Dismiss" className="shrink-0" onClick={onDismiss}>
          ×
        </button>
      ) : null}
    </FoundationToast>
  );
}

export function toast(message: string, options?: ToastOptions): number {
  return pushToast(message, options);
}

export function updateToast(id: number, patch: ToastUpdate): void {
  patchToast(id, patch);
}

export function dismissToast(id: number): void {
  removeToast(id);
}
