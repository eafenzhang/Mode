// Command palette primitive（spec: docs/specs/m4-ui-foundation.md §2.8）。
// 过滤/键盘/选中语义由 cmdk 公共 API 提供；本文件只给壳、输入行与选项视觉。

import {
  Command as CmdkCommand,
  CommandEmpty as CmdkEmpty,
  CommandGroup as CmdkGroup,
  CommandInput as CmdkInput,
  CommandItem as CmdkItem,
  CommandList as CmdkList,
  CommandSeparator as CmdkSeparator,
} from "cmdk";
import type { ComponentProps, HTMLAttributes, ReactNode } from "react";

import {
  commandClassName,
  commandEmptyClassName,
  commandGroupClassName,
  commandInputClassName,
  commandInputWrapperClassName,
  commandItemClassName,
  commandListClassName,
  commandShortcutClassName,
} from "./commandStyles.js";
import { cn } from "@/components/lib/utils.js";

export type CommandInputProps = ComponentProps<typeof CmdkInput>;
export type CommandItemProps = ComponentProps<typeof CmdkItem>;

export function Command({ className, ...rest }: ComponentProps<typeof CmdkCommand>): ReactNode {
  return <CmdkCommand {...rest} className={commandClassName(className)} />;
}

export function CommandInput({
  className,
  wrapperClassName,
  ...rest
}: CommandInputProps & { wrapperClassName?: string }): ReactNode {
  return (
    <div
      data-slot="command-input-wrapper"
      className={commandInputWrapperClassName(wrapperClassName)}
    >
      <svg
        aria-hidden="true"
        viewBox="0 0 16 16"
        className="size-4 shrink-0 text-foreground-subtle"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
      >
        <circle cx="7" cy="7" r="4.5" />
        <path d="M10.5 10.5L14 14" />
      </svg>
      <CmdkInput {...rest} data-slot="command-input" className={commandInputClassName(className)} />
    </div>
  );
}

export function CommandList({ className, ...rest }: ComponentProps<typeof CmdkList>): ReactNode {
  return <CmdkList {...rest} className={commandListClassName(className)} />;
}

export function CommandEmpty({ className, ...rest }: ComponentProps<typeof CmdkEmpty>): ReactNode {
  return <CmdkEmpty {...rest} className={commandEmptyClassName(className)} />;
}

export function CommandGroup({ className, ...rest }: ComponentProps<typeof CmdkGroup>): ReactNode {
  return <CmdkGroup {...rest} className={commandGroupClassName(className)} />;
}

export function CommandItem({ className, ...rest }: ComponentProps<typeof CmdkItem>): ReactNode {
  return <CmdkItem {...rest} className={commandItemClassName(className)} />;
}

export function CommandSeparator({
  className,
  ...rest
}: ComponentProps<typeof CmdkSeparator>): ReactNode {
  return <CmdkSeparator {...rest} className={cn(["-mx-1 my-1 h-px bg-border", className])} />;
}

export function CommandShortcut({
  className,
  ...rest
}: HTMLAttributes<HTMLSpanElement>): ReactNode {
  return (
    <span {...rest} data-slot="command-shortcut" className={commandShortcutClassName(className)} />
  );
}
