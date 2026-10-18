// P1.1 Tabs 键盘导航的纯逻辑（spec: docs/specs/m4-ui-foundation.md §2.5）。
// 只负责「下一个应聚焦的 tab 下标」；是否把焦点转成选中由调用方决定。

export type TabNavigationKey = "ArrowLeft" | "ArrowRight" | "Home" | "End";

export function isTabNavigationKey(key: string): key is TabNavigationKey {
  return key === "ArrowLeft" || key === "ArrowRight" || key === "Home" || key === "End";
}

/**
 * 返回应聚焦的 tab 下标；无 tab 时返回 null。
 * - ArrowLeft/ArrowRight 循环；
 * - Home/End 到首/尾；
 * - current 越界时按第一个处理。
 */
export function nextTabIndex(
  current: number,
  length: number,
  key: TabNavigationKey,
): number | null {
  if (length <= 0) {
    return null;
  }
  const safe = current < 0 || current >= length ? 0 : current;
  switch (key) {
    case "ArrowLeft":
      return (safe - 1 + length) % length;
    case "ArrowRight":
      return (safe + 1) % length;
    case "Home":
      return 0;
    case "End":
      return length - 1;
  }
}
