// M4 adapter 第一批：旧导出名 → ui-foundation 的包装。
// 逐屏切换时调用方从 `@/ui-foundation/adapters` 导入，props 形状保持旧命名；
// 未覆盖的旧 props 在切换该屏时再补，不提前猜测。

export {
  Badge,
  badgeVariants,
  mapBadgeVariant,
  type AdapterBadgeProps,
  type AdapterBadgeVariant,
} from "./badge.js";
export {
  Button,
  buttonVariants,
  mapButtonSize,
  mapButtonVariant,
  type AdapterButtonProps,
  type AdapterButtonSize,
  type AdapterButtonVariant,
} from "./button.js";
export {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
  type CardProps,
} from "./card.js";
export { Input, inputVariants, type AdapterInputProps } from "./input.js";
export { Separator, type SeparatorProps } from "./separator.js";
