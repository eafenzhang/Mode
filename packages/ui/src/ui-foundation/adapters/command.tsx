// M4 adapter：Command（旧 API：Command/CommandDialog/Input/List/Empty/Group/Item/Separator/Shortcut）。
// 基础件复用 foundation Command；Dialog 组合放在 adapter 以满足旧的标题/描述入参。

import { useId, type ComponentProps, type ReactNode } from "react";

import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
  CommandShortcut,
} from "../Command.js";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "./dialog.js";
import { cn } from "@/components/lib/utils.js";

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
} from "../Command.js";

export interface CommandDialogProps {
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  title?: ReactNode;
  description?: ReactNode;
  className?: string;
  children?: ReactNode;
  /** 兼容旧入参；CommandDialog 默认不显示右上角关闭按钮。 */
  showCloseButton?: boolean;
}

export function CommandDialog({
  open,
  onOpenChange,
  title,
  description,
  className,
  showCloseButton = false,
  children,
}: CommandDialogProps): ReactNode {
  const titleId = useId();
  const descriptionId = useId();
  const mergedClassName = cn(["overflow-hidden p-0", className]);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        labelledBy={title ? titleId : undefined}
        showCloseButton={showCloseButton}
        className={mergedClassName}
      >
        {title ? (
          <DialogTitle id={titleId} className="sr-only">
            {title}
          </DialogTitle>
        ) : null}
        {description ? (
          <DialogDescription id={descriptionId} className="sr-only">
            {description}
          </DialogDescription>
        ) : null}
        {children}
      </DialogContent>
    </Dialog>
  );
}

export type CommandProps = ComponentProps<typeof Command>;
