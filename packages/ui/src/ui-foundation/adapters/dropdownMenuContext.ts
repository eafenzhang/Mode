// M4 adapter：DropdownMenu 的共享状态（旧 Radix API 形状）。
import { createContext, useContext } from "react";
import type { MutableRefObject } from "react";

export interface DropdownMenuAdapterState {
  open: boolean;
  setOpen: (open: boolean) => void;
  closeMenu: () => void;
  triggerRef: MutableRefObject<HTMLElement | null>;
  contentId: string;
}

export const DropdownMenuAdapterContext = createContext<DropdownMenuAdapterState | null>(null);

export function useDropdownMenuAdapterState(): DropdownMenuAdapterState | null {
  return useContext(DropdownMenuAdapterContext);
}

export interface DropdownSubAdapterState {
  open: boolean;
  setOpen: (open: boolean) => void;
  triggerRef: MutableRefObject<HTMLElement | null>;
  contentId: string;
}

export const DropdownSubAdapterContext = createContext<DropdownSubAdapterState | null>(null);

export function useDropdownSubAdapterState(): DropdownSubAdapterState | null {
  return useContext(DropdownSubAdapterContext);
}

export interface DropdownRadioGroupAdapterState {
  value: string | undefined;
  setValue: (value: string) => void;
}

export const DropdownRadioGroupAdapterContext =
  createContext<DropdownRadioGroupAdapterState | null>(null);

export function useDropdownRadioGroupAdapterState(): DropdownRadioGroupAdapterState | null {
  return useContext(DropdownRadioGroupAdapterContext);
}

export interface MenuSelectEvent {
  defaultPrevented: boolean;
  preventDefault: () => void;
}

export function menuSelectEvent(): MenuSelectEvent {
  const event = { defaultPrevented: false, preventDefault: () => (event.defaultPrevented = true) };
  return event;
}

export const MENU_ITEM_SELECTOR =
  '[role="menuitem"]:not([disabled]), [role="menuitemcheckbox"]:not([disabled]), [role="menuitemradio"]:not([disabled])';

export function menuItemNodes(container: HTMLElement | null): HTMLElement[] {
  return container ? [...container.querySelectorAll<HTMLElement>(MENU_ITEM_SELECTOR)] : [];
}

export function moveMenuFocus(container: HTMLElement | null, delta: 1 | -1): void {
  const items = menuItemNodes(container);
  if (items.length === 0) {
    return;
  }
  const current = items.indexOf(document.activeElement as HTMLElement);
  const next =
    delta === 1
      ? (current + 1 + items.length) % items.length
      : (current - 1 + items.length) % items.length;
  items[next]?.focus();
}
