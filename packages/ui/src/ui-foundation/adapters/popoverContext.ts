// M4 adapter：Popover 的共享状态与旧 Radix 事件形状。
import { createContext, useContext } from "react";
import type { MutableRefObject } from "react";

// 旧调用点既有真实元素锚点，也有只提供 getBoundingClientRect 的 virtualRef。
export interface PopoverAnchorTarget {
  getBoundingClientRect(): DOMRect;
  contains?(node: Node): boolean;
  focus?(): void;
}

export interface PopoverAdapterState {
  open: boolean;
  setOpen: (open: boolean) => void;
  anchorRef: MutableRefObject<PopoverAnchorTarget | null>;
  contentId: string;
}

export const PopoverAdapterContext = createContext<PopoverAdapterState | null>(null);

export function usePopoverAdapterState(): PopoverAdapterState | null {
  return useContext(PopoverAdapterContext);
}

export interface PopoverEvent {
  target: EventTarget | null;
  currentTarget: HTMLElement | null;
  preventDefault: () => void;
}

export interface PopoverPointerEvent extends PopoverEvent {
  detail: { originalEvent: globalThis.MouseEvent };
}

export interface PopoverFocusEvent extends PopoverEvent {
  isComposing?: boolean;
}

export interface PreventableEvent {
  defaultPrevented: boolean;
  preventDefault: () => void;
}

export function preventableEvent(): PreventableEvent {
  const event = { defaultPrevented: false, preventDefault: () => (event.defaultPrevented = true) };
  return event;
}

export const POPOVER_FOCUSABLE_SELECTOR = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  '[tabindex]:not([tabindex="-1"])',
].join(",");
