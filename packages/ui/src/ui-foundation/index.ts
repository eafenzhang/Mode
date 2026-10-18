/* oxlint-disable eslint(max-lines) -- ui-foundation 的公开导出 barrel；按组件/样式分组导出会拆出额外的空壳文件。 */
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
} from "./Avatar.js";
export { Alert, AlertAction, AlertDescription, AlertTitle, type AlertProps } from "./Alert.js";
export {
  AppShell,
  ResizeHandle,
  type AppShellProps,
  type PanelResizeBinding,
  type ResizeHandleProps,
} from "./AppShell.js";
export { ApprovalBlock, type ApprovalBlockProps } from "./ApprovalBlock.js";
export { Button, type ButtonProps } from "./Button.js";
export { Checkbox, type CheckboxProps } from "./Checkbox.js";
export { DiffBlock, DiffLine, type DiffBlockProps, type DiffLineProps } from "./DiffBlock.js";
export {
  ChatInputShell,
  EmptyChatState,
  MessageList,
  MessageRow,
  StreamingIndicator,
  type ChatInputShellProps,
  type EmptyChatStateProps,
  type MessageRowProps,
  type StreamingIndicatorProps,
} from "./Chat.js";
export {
  chatInputShellClassName,
  chatInputToolbarClassName,
  emptyDescriptionClassName,
  emptyStateClassName,
  emptyTitleClassName,
  messageBubbleClassName,
  messageListClassName,
  messageRowClassName,
  streamingIndicatorClassName,
  type ChatRole,
} from "./chatStyles.js";
export {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
  type CardProps,
} from "./Card.js";
export { Badge, type BadgeProps } from "./Badge.js";
export {
  Dialog,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  type DialogProps,
} from "./Dialog.js";
export { Input, type InputProps } from "./Input.js";
export {
  Menu,
  MenuCheckboxItem,
  MenuGroup,
  MenuItem,
  MenuRadioItem,
  MenuSeparator,
  type MenuCheckboxItemProps,
  type MenuGroupProps,
  type MenuItemProps,
  type MenuProps,
  type MenuRadioItemProps,
} from "./Menu.js";
export { OnboardingShell, type OnboardingShellProps } from "./Onboarding.js";
export { Popover, type PopoverProps } from "./Popover.js";
export { Progress, type ProgressProps } from "./Progress.js";
export { Radio, RadioGroup, type RadioGroupProps, type RadioProps } from "./RadioGroup.js";
export {
  QueuedInputItem,
  QueuedInputList,
  type QueuedInputItemProps,
  type QueuedInputListProps,
} from "./QueuedInput.js";
export {
  PreviewPane,
  RemoteStatusItem,
  RemoteSurface,
  RemoteToolbarButton,
  TerminalPane,
  type PreviewPaneProps,
  type RemoteStatusItemProps,
  type RemoteSurfaceProps,
  type RemoteToolbarButtonProps,
  type TerminalPaneProps,
} from "./Remote.js";
export {
  QuickpickEmpty,
  QuickpickInput,
  QuickpickItem,
  QuickpickList,
  QuickpickPanel,
  type QuickpickItemProps,
  type QuickpickListProps,
} from "./Quickpick.js";
export {
  ShortcutList,
  ShortcutRow,
  type ShortcutListProps,
  type ShortcutRowProps,
} from "./Shortcuts.js";
export { ScrollArea, ScrollBar, type ScrollAreaProps, type ScrollBarProps } from "./ScrollArea.js";
export { SelectTrigger, type SelectTriggerProps } from "./SelectTrigger.js";
export { Separator, type SeparatorProps } from "./Separator.js";
export {
  SelectContent,
  SelectGroup,
  SelectGroupLabel,
  SelectItem,
  SelectValue,
  type SelectContentProps,
  type SelectGroupProps,
  type SelectItemProps,
  type SelectValueProps,
} from "./Select.js";
export {
  PluginCard,
  PluginUnavailableHint,
  SettingsLayout,
  SettingsNavItem,
  SettingsSection,
  type PluginCardProps,
  type PluginUnavailableHintProps,
  type SettingsLayoutProps,
  type SettingsNavItemProps,
  type SettingsSectionProps,
} from "./Settings.js";
export { Switch, type SwitchProps } from "./Switch.js";
export { Tab, Tabs, type TabProps, type TabsProps } from "./Tabs.js";
export { isTabNavigationKey, nextTabIndex, type TabNavigationKey } from "./tabsKeyboard.js";
export { Textarea, type TextareaProps } from "./Textarea.js";
export { Toast, type ToastProps } from "./Toast.js";
export { ToolBlock, type ToolBlockProps } from "./ToolBlock.js";
export {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
  type TooltipProps,
  type TooltipTriggerProps,
} from "./Tooltip.js";
export { TooltipProvider, type TooltipProviderProps } from "./TooltipProvider.js";
export {
  DEFAULT_TOOLTIP_DELAY_MS,
  MAX_TOOLTIP_DELAY_MS,
  resolveTooltipDelay,
  useTooltipConfig,
  type TooltipConfig,
} from "./tooltipContext.js";
export {
  FileTree,
  FileTreeItem,
  TaskList,
  TaskRow,
  TaskStatusBadge,
  type FileTreeItemProps,
  type FileTreeProps,
  type TaskListProps,
  type TaskRowProps,
  type TaskStatusBadgeProps,
} from "./Workspace.js";
export { WorkspaceHeader, type WorkspaceHeaderProps } from "./WorkspaceHeader.js";
export { controlRadiusClass } from "./radius.js";
export {
  buttonClassName,
  type ButtonClassNameOptions,
  type ButtonSize,
  type ButtonVariant,
} from "./buttonStyles.js";
export {
  inputClassName,
  selectTriggerClassName,
  textareaClassName,
  type FieldClassNameOptions,
  type FieldSize,
  type SelectTriggerClassNameOptions,
} from "./inputStyles.js";
export {
  avatarBadgeClassName,
  avatarClassName,
  avatarFallbackClassName,
  avatarGroupClassName,
  avatarImageClassName,
  type AvatarBadgeTone,
  type AvatarSize,
} from "./avatarStyles.js";
export {
  scrollAreaClassName,
  scrollAreaViewportClassName,
  scrollBarClassName,
  type ScrollOrientation,
} from "./scrollStyles.js";
export {
  alertActionClassName,
  alertClassName,
  alertDescriptionClassName,
  alertTitleClassName,
  type AlertClassNameOptions,
  type AlertTone,
} from "./alertStyles.js";
export {
  checkboxClassName,
  checkboxRowClassName,
  progressIndicatorClassName,
  progressPercent,
  progressTrackClassName,
  radioClassName,
  radioGroupClassName,
  radioRowClassName,
  switchThumbClassName,
  switchTrackClassName,
} from "./formStyles.js";
export { badgeClassName, type BadgeClassNameOptions, type BadgeTone } from "./badgeStyles.js";
export { separatorClassName, type SeparatorOrientation } from "./separatorStyles.js";
export {
  cardActionClassName,
  cardClassName,
  cardContentClassName,
  cardDescriptionClassName,
  cardFooterClassName,
  cardHeaderClassName,
  cardTitleClassName,
  containerRadiusClass,
  dialogClassName,
  menuClassName,
  menuGroupClassName,
  menuLabelClassName,
  menuItemClassName,
  dialogHeaderClassName,
  dialogTitleClassName,
  dialogDescriptionClassName,
  dialogFooterClassName,
  popoverClassName,
  type CardClassNameOptions,
  type MenuItemClassNameOptions,
} from "./surfaceStyles.js";
export {
  selectContentClassName,
  selectGroupLabelClassName,
  selectItemClassName,
  selectValueClassName,
  type SelectValueClassNameOptions,
} from "./selectStyles.js";
export {
  tabClassName,
  tabListClassName,
  toastClassName,
  tooltipClassName,
  tooltipShortcutClassName,
  type TabClassNameOptions,
  type ToastClassNameOptions,
} from "./feedbackStyles.js";
export {
  conversationColumnClassName,
  conversationFrameClassName,
  mobileDrawerClassName,
  resizeHandleClassName,
  sidePaneFrameClassName,
  terminalFrameClassName,
  workspaceShellClassName,
  type ResizeOrientation,
} from "./shellStyles.js";
export {
  workspaceHeaderActionsClassName,
  workspaceHeaderClassName,
  workspaceHeaderLeadingClassName,
  workspaceHeaderSubtitleClassName,
  workspaceHeaderTitleClassName,
} from "./headerStyles.js";
export {
  approvalActionsClassName,
  approvalBlockClassName,
  approvalDescriptionClassName,
  approvalStatusClassName,
  approvalTitleClassName,
  toolBlockClassName,
  toolBodyClassName,
  toolHeaderClassName,
  toolNameClassName,
  toolStatusClassName,
  type ApprovalStatus,
  type ToolBlockState,
} from "./toolStyles.js";
export {
  diffBlockClassName,
  diffFilePathClassName,
  diffHeaderClassName,
  diffLineClassName,
  diffMarker,
  diffStatsClassName,
  queuedInputActionsClassName,
  queuedInputItemClassName,
  queuedInputListClassName,
  queuedInputTextClassName,
  type DiffLineKind,
} from "./diffStyles.js";
export {
  FILE_TREE_INDENT_BASE_PX,
  FILE_TREE_INDENT_STEP_PX,
  fileTreeClassName,
  fileTreeIconClassName,
  fileTreeItemClassName,
  fileTreeLabelClassName,
  taskListClassName,
  taskRowClassName,
  taskRowMetaClassName,
  taskRowTitleClassName,
  taskStatusBadgeClassName,
  taskStatusDotClassName,
  type FileTreeItemClassNameOptions,
  type TaskRowClassNameOptions,
  type TaskStatus,
} from "./workspaceStyles.js";
export {
  pluginAvailabilityBadgeClassName,
  pluginCardActionsClassName,
  pluginCardBodyClassName,
  pluginCardClassName,
  pluginCardDescriptionClassName,
  pluginCardHeaderClassName,
  pluginCardTitleClassName,
  pluginDetailHintClassName,
  pluginIconClassName,
  settingsContentClassName,
  settingsLayoutClassName,
  settingsNavClassName,
  settingsNavItemClassName,
  settingsSectionClassName,
  settingsSectionDescriptionClassName,
  settingsSectionTitleClassName,
  type PluginAvailability,
  type SettingsNavItemClassNameOptions,
} from "./settingsStyles.js";
export {
  clampPanelSize,
  isResizeKey,
  normalizeBounds,
  pointerDelta,
  resizeFromKeyboard,
  resizeFromPointer,
  RESIZE_KEYBOARD_STEP,
  type ResizeBounds,
  type ResizeKey,
} from "./resizeMath.js";
export {
  usePanelResize,
  type PanelResize,
  type PanelResizeHandleProps,
  type UsePanelResizeOptions,
} from "./usePanelResize.js";
export {
  createMemoryPanelSizePersistence,
  resolveInitialPanelSize,
  type PanelId,
  type PanelSizePersistence,
} from "./panelSizePersistence.js";
export { WorkspaceFrame, type WorkspaceFrameProps } from "./WorkspaceFrame.js";
export {
  previewHeaderClassName,
  previewPaneClassName,
  remoteCanvasClassName,
  remoteLayoutClassName,
  remoteOfflineOverlayClassName,
  remoteStatusItemClassName,
  remoteStatusStripClassName,
  remoteSurfaceClassName,
  remoteToolbarButtonClassName,
  remoteToolbarClassName,
  terminalPaneClassName,
  type RemoteToolbarButtonClassNameOptions,
} from "./remoteStyles.js";
export {
  onboardingActionsClassName,
  onboardingCardClassName,
  onboardingContainerClassName,
  onboardingDescriptionClassName,
  onboardingStepDotClassName,
  onboardingStepLabelClassName,
  onboardingTitleClassName,
  type OnboardingStepState,
} from "./onboardingStyles.js";
export {
  quickpickEmptyClassName,
  quickpickInputClassName,
  quickpickItemClassName,
  quickpickListClassName,
  quickpickPanelClassName,
  type QuickpickItemClassNameOptions,
} from "./quickpickStyles.js";
export {
  shortcutKeyClassName,
  shortcutKeysClassName,
  shortcutLabelClassName,
  shortcutListClassName,
  shortcutRowClassName,
} from "./shortcutStyles.js";
export {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
  type CollapsibleContentProps,
  type CollapsibleProps,
  type CollapsibleTriggerProps,
} from "./Collapsible.js";
export { Spinner, type SpinnerProps } from "./Spinner.js";
export {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
  CommandShortcut,
  type CommandInputProps,
  type CommandItemProps,
} from "./Command.js";
export { Label, type LabelProps } from "./Label.js";
export { Kbd, KbdGroup, type KbdGroupProps, type KbdProps } from "./Kbd.js";
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
} from "./InputGroup.js";
export { ScrollFadeViewport, type ScrollFadeViewportProps } from "./ScrollFadeViewport.js";
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
} from "./Chart.js";
