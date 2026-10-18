// P1.1 ScrollFadeViewport（spec: docs/specs/m4-ui-foundation.md §2.12）。
// ref 指向真实滚动元素；仅按溢出方向给 1rem 渐变遮罩，不拦截事件。

import {
  forwardRef,
  useCallback,
  useEffect,
  useRef,
  useState,
  type ComponentProps,
  type UIEvent,
} from "react";

type FadeState = { top: boolean; bottom: boolean };

const FADE_CLASS: Record<"none" | "top" | "bottom" | "both", string> = {
  none: "",
  top: "[mask-image:linear-gradient(to_bottom,transparent,black_1rem)]",
  bottom: "[mask-image:linear-gradient(to_bottom,black_calc(100%-1rem),transparent)]",
  both: "[mask-image:linear-gradient(to_bottom,transparent,black_1rem,black_calc(100%-1rem),transparent)]",
};

function fadeKey(state: FadeState): keyof typeof FADE_CLASS {
  if (state.top && state.bottom) {
    return "both";
  }
  if (state.top) {
    return "top";
  }
  if (state.bottom) {
    return "bottom";
  }
  return "none";
}

export interface ScrollFadeViewportProps extends ComponentProps<"div"> {}

export const ScrollFadeViewport = forwardRef<HTMLDivElement, ScrollFadeViewportProps>(
  function ScrollFadeViewport({ className, onScroll, children, ...rest }, ref) {
    const innerRef = useRef<HTMLDivElement | null>(null);
    const [fade, setFade] = useState<FadeState>({ top: false, bottom: false });

    const assignRef = useCallback(
      (node: HTMLDivElement | null): void => {
        innerRef.current = node;
        if (typeof ref === "function") {
          ref(node);
        } else if (ref) {
          ref.current = node;
        }
      },
      [ref],
    );

    const update = useCallback(() => {
      const node = innerRef.current;
      if (!node) {
        return;
      }
      const top = node.scrollTop > 1;
      const bottom = node.scrollTop + node.clientHeight < node.scrollHeight - 1;
      setFade((previous) =>
        previous.top === top && previous.bottom === bottom ? previous : { top, bottom },
      );
    }, []);

    useEffect(() => {
      const node = innerRef.current;
      if (!node || typeof ResizeObserver === "undefined") {
        update();
        return;
      }
      update();
      const observer = new ResizeObserver(update);
      observer.observe(node);
      for (const child of node.children) {
        observer.observe(child);
      }
      return () => observer.disconnect();
    }, [update, children]);

    const handleScroll = (event: UIEvent<HTMLDivElement>): void => {
      onScroll?.(event);
      update();
    };

    return (
      <div
        {...rest}
        ref={assignRef}
        data-slot="scroll-fade-viewport"
        data-fade-top={fade.top ? "" : undefined}
        data-fade-bottom={fade.bottom ? "" : undefined}
        onScroll={handleScroll}
        className={[className, FADE_CLASS[fadeKey(fade)]].filter(Boolean).join(" ")}
      >
        {children}
      </div>
    );
  },
);
