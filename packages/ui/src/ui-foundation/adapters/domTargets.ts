// M4 adapter 共享：旧组件 asChild 场景的 ref 合并。
import type { ReactElement, Ref } from "react";

export function mergeRefs<T>(...refs: Array<Ref<T> | undefined>): (value: T | null) => void {
  return (value) => {
    for (const ref of refs) {
      if (typeof ref === "function") {
        ref(value);
      } else if (ref) {
        (ref as { current: T | null }).current = value;
      }
    }
  };
}

export function childRefOf<T>(child: ReactElement): Ref<T> | undefined {
  return (child.props as { ref?: Ref<T> }).ref;
}
