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
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogOverlay,
  AlertDialogPortal,
  AlertDialogTitle,
  AlertDialogTrigger,
  type AlertDialogActionProps,
  type AlertDialogCancelProps,
  type AlertDialogContentProps,
  type AlertDialogProps,
  type AlertDialogTriggerProps,
} from "./alert-dialog.js";
export {
  Alert,
  AlertAction,
  AlertDescription,
  AlertTitle,
  mapAlertVariant,
  type AdapterAlertProps,
  type AdapterAlertVariant,
} from "./alert.js";
export {
  Avatar,
  AvatarBadge,
  AvatarFallback,
  AvatarGroup,
  AvatarGroupCount,
  AvatarImage,
  type AvatarBadgeProps,
  type AvatarFallbackProps,
  type AvatarGroupCountProps,
  type AvatarGroupProps,
  type AvatarImageProps,
  type AvatarProps,
} from "./avatar.js";
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
export {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogPortal,
  DialogTitle,
  DialogTrigger,
  type DialogCloseProps,
  type DialogContentProps,
  type DialogProps,
  type DialogTriggerProps,
} from "./dialog.js";
export { Checkbox, type CheckboxProps } from "./checkbox.js";
export { Input, inputVariants, type AdapterInputProps } from "./input.js";
export { Progress, type ProgressProps } from "./progress.js";
export { Radio, RadioGroup, type RadioGroupProps, type RadioProps } from "./radio-group.js";
export { Switch, type SwitchProps } from "./switch.js";
export { Textarea, type TextareaProps } from "./textarea.js";
export { Separator, type SeparatorProps } from "./separator.js";
export { ScrollArea, ScrollBar, type ScrollAreaProps, type ScrollBarProps } from "./scroll-area.js";
export {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuPortal,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
  type DropdownMenuCheckboxItemProps,
  type DropdownMenuContentProps,
  type DropdownMenuItemProps,
  type DropdownMenuLabelProps,
  type DropdownMenuProps,
  type DropdownMenuRadioGroupProps,
  type DropdownMenuRadioItemProps,
  type DropdownMenuSubContentProps,
  type DropdownMenuSubProps,
  type DropdownMenuSubTriggerProps,
  type DropdownMenuTriggerProps,
} from "./dropdown-menu.js";
export {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectSeparator,
  SelectTrigger,
  SelectValue,
  type SelectContentProps,
  type SelectGroupProps,
  type SelectItemProps,
  type SelectProps,
  type SelectTriggerProps,
  type SelectValueProps,
} from "./select.js";
export {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
  type TooltipContentProps,
  type TooltipProps,
  type TooltipProviderProps,
  type TooltipTriggerProps,
} from "./tooltip.js";
export {
  Popover,
  PopoverAnchor,
  PopoverContent,
  PopoverDescription,
  PopoverHeader,
  PopoverTitle,
  PopoverTrigger,
  type PopoverAnchorProps,
  type PopoverContentProps,
  type PopoverProps,
  type PopoverTriggerProps,
} from "./popover.js";
export {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
  tabsListVariants,
  type TabsContentProps,
  type TabsListProps,
  type TabsProps,
  type TabsTriggerProps,
} from "./tabs.js";
export {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
  type CollapsibleContentProps,
  type CollapsibleProps,
  type CollapsibleTriggerProps,
} from "./collapsible.js";
export { Spinner, type SpinnerProps } from "./spinner.js";
export {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
  type ContextMenuContentProps,
  type ContextMenuItemProps,
  type ContextMenuProps,
  type ContextMenuSubContentProps,
  type ContextMenuSubProps,
  type ContextMenuSubTriggerProps,
  type ContextMenuTriggerProps,
} from "./context-menu.js";
export {
  Command,
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
  CommandShortcut,
  type CommandDialogProps,
  type CommandInputProps,
  type CommandItemProps,
} from "./command.js";
export { Label, type LabelProps } from "./label.js";
export { Kbd, KbdGroup, type KbdGroupProps, type KbdProps } from "./kbd.js";
export {
  HoverCard,
  HoverCardContent,
  HoverCardTrigger,
  type HoverCardContentProps,
  type HoverCardProps,
  type HoverCardTriggerProps,
} from "./hover-card.js";
export {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
  InputGroupText,
  InputGroupTextarea,
  type InputGroupAddonAlign,
  type InputGroupAddonProps,
  type InputGroupButtonProps,
  type InputGroupInputProps,
  type InputGroupProps,
  type InputGroupTextareaProps,
  type InputGroupTextProps,
} from "./input-group.js";
export { ScrollFadeViewport, type ScrollFadeViewportProps } from "./scroll-fade-viewport.js";
export {
  ChartContainer,
  ChartLegend,
  ChartLegendContent,
  ChartStyle,
  ChartTooltip,
  ChartTooltipContent,
  useChart,
  type ChartConfig,
  type ChartConfigEntry,
  type ChartContainerProps,
  type ChartLegendContentProps,
  type ChartLegendPayloadItem,
  type ChartTooltipContentProps,
  type ChartTooltipPayloadItem,
} from "./chart.js";
export {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup,
  type ResizableHandleProps,
  type ResizablePanelGroupProps,
  type ResizablePanelProps,
} from "./resizable.js";
export {
  DiffViewer,
  type DiffViewerProps,
  type DiffViewerSelectedLineRange,
} from "./diff-viewer.js";
export {
  LightweightDiffPreview,
  getLightweightDiffLineParts,
  type LightweightDiffLineKind,
  type LightweightDiffLineParts,
  type LightweightDiffPreviewProps,
} from "./lightweight-diff-preview.js";
export {
  HighlightedLightweightDiffPreview,
  buildHighlightedLightweightDiffCode,
  getHighlightedLightweightDiffLine,
  type HighlightedLightweightDiffPreviewProps,
} from "./highlighted-lightweight-diff-preview.js";
