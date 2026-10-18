// M4 UI 基础层（P1.1）的纯逻辑契约：圆角解析与排版 token 守卫。
//
// 规格：docs/specs/m4-ui-foundation.md（由 DESIGN.md 收敛）。
// 本文件只包含可单测的纯函数，不引用旧组件实现；后续 primitives 复用这里的解析结果。

export type RadiusLevel = "none" | "sm" | "md" | "lg" | "xl" | "2xl";

/** `text-ui-*` 的合法取值（`2xs` 仅限工作流时间轴轴标签，类名层无法判定使用位置）。 */
export const UI_TEXT_TOKENS = [
  "text-ui-2xs",
  "text-ui-xs",
  "text-ui-sm",
  "text-ui-caption",
  "text-ui-base",
  "text-ui-lg",
  "text-ui-xl",
] as const;

/**
 * 普通控件（Button/Input/Textarea/Select trigger）相对最近圆角父容器的半径。
 * 规则：无父容器或父为 xl 及以上 → lg；父 lg → md；父 md/sm → sm。
 */
export function resolveControlRadius(parent: RadiusLevel): RadiusLevel {
  switch (parent) {
    case "none":
    case "xl":
    case "2xl":
      return "lg";
    case "lg":
      return "md";
    case "md":
    case "sm":
      return "sm";
  }
}

/**
 * 嵌套圆角容器的下一级半径。
 * 规则：第一个可见圆角容器从 xl 起；Dialog shell（2xl）不计层级，内容重新从 xl 起；
 * sm 是下限。
 */
export function resolveNestedRadius(parent: RadiusLevel): RadiusLevel {
  switch (parent) {
    case "none":
    case "2xl":
      return "xl";
    case "xl":
      return "lg";
    case "lg":
      return "md";
    case "md":
    case "sm":
      return "sm";
  }
}

/**
 * 扫描 className 字符串里的排版违规：
 * - Tailwind 内置 `text-base` / `text-sm` / `text-xs`；
 * - 任意字号 `text-[13px]` / `text-[0.9rem]`；
 * - 未定义的 `text-ui-*` token。
 *
 * 颜色/其它 `text-*` 工具类（如 `text-foreground`）不在此守卫范围。
 */
export function findUiTypographyViolations(classNames: readonly string[]): string[] {
  const violations: string[] = [];
  const allowed = new Set<string>(UI_TEXT_TOKENS);
  for (const className of classNames) {
    for (const token of className.split(/\s+/).filter(Boolean)) {
      if (/^text-(base|sm|xs)$/.test(token)) {
        violations.push(token);
        continue;
      }
      if (/^text-\[.+\]$/.test(token) && /(px|rem|em)$/.test(token.replace(/^text-\[|\]$/g, ""))) {
        violations.push(token);
        continue;
      }
      if (token.startsWith("text-ui-") && !allowed.has(token)) {
        violations.push(token);
      }
    }
  }
  return violations;
}
