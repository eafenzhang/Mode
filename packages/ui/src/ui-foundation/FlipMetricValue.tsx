// P1.1 FlipMetricValue（spec: docs/specs/m4-ui-foundation.md §2.17）。
// 逐字符显示指标；值变化时仅让变化的字符做一次性淡入/上移动画。

import { useEffect, useMemo, useRef, useState } from "react";

function usePrefersReducedMotion(): boolean {
  const [reduced, setReduced] = useState(false);
  useEffect(() => {
    if (typeof window === "undefined" || !window.matchMedia) {
      return;
    }
    const query = window.matchMedia("(prefers-reduced-motion: reduce)");
    setReduced(query.matches);
    const handleChange = (event: MediaQueryListEvent): void => setReduced(event.matches);
    query.addEventListener("change", handleChange);
    return () => query.removeEventListener("change", handleChange);
  }, []);
  return reduced;
}

function characterWidthClass(character: string): string {
  if (character === " ") {
    return "w-[0.3em]";
  }
  if (character === "." || character === ",") {
    return "w-[0.45em]";
  }
  return "w-[1ch]";
}

export interface FlipMetricValueProps {
  value: string;
  className?: string;
  animateInitial?: boolean;
}

export function FlipMetricValue({
  value,
  className,
  animateInitial = false,
}: FlipMetricValueProps) {
  const reducedMotion = usePrefersReducedMotion();
  const characters = useMemo(() => Array.from(value), [value]);
  const previousRef = useRef(value);
  const changedFlags = useMemo(
    () =>
      characters.map(
        (character, index) => animateInitial || previousRef.current[index] !== character,
      ),
    [characters, animateInitial],
  );

  useEffect(() => {
    previousRef.current = value;
  }, [value]);

  return (
    <span
      aria-label={value}
      data-animate-initial={animateInitial ? "true" : undefined}
      className={[
        "inline-flex max-w-full items-center overflow-hidden whitespace-nowrap align-middle leading-none",
        "tabular-nums",
        className,
      ]
        .filter(Boolean)
        .join(" ")}
    >
      {characters.map((character, index) => (
        <span
          key={`${index}-${character}`}
          aria-hidden="true"
          className={[
            "inline-flex justify-center",
            characterWidthClass(character),
            !reducedMotion && changedFlags[index]
              ? "animate-in fade-in-0 slide-in-from-bottom-1 duration-150"
              : "",
          ]
            .filter(Boolean)
            .join(" ")}
        >
          {character === " " ? "\u00a0" : character}
        </span>
      ))}
    </span>
  );
}
