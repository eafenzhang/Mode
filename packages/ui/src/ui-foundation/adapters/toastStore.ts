// M4 adapter：toast 的命令式队列（与 React 无关，便于单测）。

export type ToastPosition = "top-center" | "top-right" | "bottom-left" | "bottom-center";
export type ToastVariant = "default" | "update" | "info" | "warning";

export interface ToastOptions {
  durationMs?: number;
  position?: ToastPosition;
  variant?: ToastVariant;
  actionLabel?: string;
  onAction?: () => void;
  dismissible?: boolean;
  dismissLabel?: string;
  anchorId?: string;
  /** 同一业务目标与动作的稳定键；新结果替换旧结果并移到栈底。 */
  dedupeKey?: string;
}

export interface ToastItem {
  id: number;
  message: string;
  durationMs: number;
  position: ToastPosition;
  variant?: ToastVariant;
  actionLabel?: string;
  onAction?: () => void;
  dismissible?: boolean;
  dismissLabel?: string;
  anchorId?: string;
  dedupeKey?: string;
}

export type ToastUpdate = Partial<Omit<ToastItem, "id">>;

export const DEFAULT_TOAST_DURATION_MS = 3000;

export function upsertToastItem(items: readonly ToastItem[], item: ToastItem): ToastItem[] {
  if (!item.dedupeKey) {
    return [...items, item];
  }
  return [...items.filter((current) => current.dedupeKey !== item.dedupeKey), item];
}

export function resolveToastStackClassName(position: ToastPosition): string {
  const bottomInset = "bottom-[calc(1rem+env(safe-area-inset-bottom))]";
  switch (position) {
    case "top-center":
      return "fixed top-16 start-1/2 z-[9999] flex -translate-x-1/2 rtl:translate-x-1/2 flex-col items-center gap-2";
    case "top-right":
      return "fixed end-4 top-16 z-[9999] flex max-w-[calc(100vw-2rem)] flex-col items-end gap-2";
    case "bottom-center":
      return `fixed ${bottomInset} start-1/2 z-[9999] flex -translate-x-1/2 rtl:translate-x-1/2 flex-col items-center gap-2`;
    case "bottom-left":
      return `fixed ${bottomInset} start-4 z-[9999] flex flex-col items-start gap-2`;
  }
}

export function resolveToastAnchorLeft(rect: Pick<DOMRect, "left" | "width">): number {
  return rect.left + rect.width / 2;
}

type Listener = (items: readonly ToastItem[]) => void;

let nextId = 0;
let items: ToastItem[] = [];
const listeners = new Set<Listener>();
type HostListener = () => void;
const hostListeners = new Set<HostListener>();

function emit(): void {
  for (const listener of listeners) {
    listener(items);
  }
}

export function subscribeToasts(listener: Listener): () => void {
  listeners.add(listener);
  listener(items);
  return () => listeners.delete(listener);
}

export function onToastHostRequest(listener: HostListener): () => void {
  hostListeners.add(listener);
  return () => hostListeners.delete(listener);
}

function requestHost(): void {
  for (const listener of hostListeners) {
    listener();
  }
}

export function getToastItems(): readonly ToastItem[] {
  return items;
}

export function pushToast(message: string, options: ToastOptions = {}): number {
  const id = nextId++;
  const item: ToastItem = {
    id,
    message,
    durationMs: options.durationMs ?? DEFAULT_TOAST_DURATION_MS,
    position: options.position ?? "bottom-center",
    variant: options.variant,
    actionLabel: options.actionLabel,
    onAction: options.onAction,
    dismissible: options.dismissible,
    dismissLabel: options.dismissLabel,
    anchorId: options.anchorId,
    dedupeKey: options.dedupeKey,
  };
  items = upsertToastItem(items, item);
  emit();
  requestHost();
  return id;
}

export function patchToast(id: number, patch: ToastUpdate): void {
  items = items.map((item) => (item.id === id ? { ...item, ...patch } : item));
  emit();
}

export function removeToast(id: number): void {
  items = items.filter((item) => item.id !== id);
  emit();
}

/** 仅测试用：重置队列。 */
export function resetToasts(): void {
  items = [];
  nextId = 0;
  emit();
}
