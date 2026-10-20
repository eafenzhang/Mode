// M4 adapter：DropdownMenu 的共享状态（旧 Radix API 形状）。
import { createContext, useContext } from "react";
import type { MutableRefObject } from "react";

export interface DropdownMenuAdapterState {
  open: boolean;
  setOpen: (open: boolean) => void;
  closeMenu: () => void;
  triggerRef: MutableRefObject<HTMLElement | null>;
  contentId: string;
  /**
   * 当前打开的子菜单 id（根菜单内唯一）。子菜单在指针进入时把自己设为 active，
   * 其它子菜单据此关闭——否则从一个 provider 移到另一个时旧子菜单仍覆盖新内容。
   */
  activeSubmenuId: string | null;
  setActiveSubmenuId: (id: string | null) => void;
}

export const DropdownMenuAdapterContext = createContext<DropdownMenuAdapterState | null>(null);

export function useDropdownMenuAdapterState(): DropdownMenuAdapterState | null {
  return useContext(DropdownMenuAdapterContext);
}

/** 兄弟子菜单互斥判据：根菜单的 active 指向另一个子菜单时，当前子菜单必须关闭。 */
export function isSiblingSubmenuActive(
  activeSubmenuId: string | null | undefined,
  submenuId: string,
): boolean {
  return Boolean(activeSubmenuId && activeSubmenuId !== submenuId);
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

export type MenuSelectEvent = Event;

/** Radix 的 onSelect/焦点回调都收到可取消的 Event；用原生 Event 保持调用点类型兼容。 */
export function menuSelectEvent(): MenuSelectEvent {
  return new Event("menu-select", { cancelable: true });
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

/**
 * 菜单内点击判定：子菜单内容被 portal 到父菜单 DOM 之外，父菜单的 mousedown
 * 外部点击处理会把子菜单项当成“外部”并在 click 前关闭整棵菜单，导致
 * `onSelect` 永不触发。子菜单内容携带顶层菜单的 `data-dropdown-menu-root`，
 * 两处外部点击处理都按根 id 判定“仍在菜单内”。
 */
export function isInsideMenuPortal(
  target: Node | null,
  rootContentId: string | undefined,
): boolean {
  if (!target || !rootContentId) {
    return false;
  }
  const element = (target as Element).closest?.("[data-dropdown-menu-root]");
  return element?.getAttribute("data-dropdown-menu-root") === rootContentId;
}
