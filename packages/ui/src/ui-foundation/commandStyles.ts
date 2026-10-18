// Command palette 视觉契约（spec: docs/specs/m4-ui-foundation.md §2.8）。

function join(parts: Array<string | false | undefined>): string {
  return parts.filter(Boolean).join(" ");
}

export function commandClassName(className?: string): string {
  return join([
    "flex h-full w-full flex-col overflow-hidden rounded-lg bg-popover text-foreground",
    className,
  ]);
}

export function commandInputWrapperClassName(className?: string): string {
  return join(["flex h-9 items-center gap-2 border-b border-border px-3", className]);
}

export function commandInputClassName(className?: string): string {
  return join([
    "h-9 w-full min-w-0 bg-transparent text-ui-base outline-none",
    "placeholder:text-foreground-subtlest disabled:cursor-not-allowed disabled:opacity-50",
    className,
  ]);
}

export function commandListClassName(className?: string): string {
  return join(["max-h-80 min-h-0 overflow-y-auto overflow-x-hidden p-1", className]);
}

export function commandEmptyClassName(className?: string): string {
  return join(["py-6 text-center text-ui-sm text-foreground-subtlest", className]);
}

export function commandGroupClassName(className?: string): string {
  return join([
    "overflow-hidden p-1 text-foreground",
    "[&_[cmdk-group-heading]]:px-2 [&_[cmdk-group-heading]]:py-1.5",
    "[&_[cmdk-group-heading]]:text-ui-xs [&_[cmdk-group-heading]]:font-medium",
    "[&_[cmdk-group-heading]]:text-foreground-subtlest",
    className,
  ]);
}

export function commandItemClassName(className?: string): string {
  return join([
    "relative flex cursor-default select-none items-center gap-2 rounded-md px-2 py-1.5 text-ui-base outline-none",
    "data-[selected=true]:bg-menu-hover data-[selected=true]:text-foreground",
    "data-[disabled=true]:pointer-events-none data-[disabled=true]:opacity-50",
    className,
  ]);
}

export function commandShortcutClassName(className?: string): string {
  return join(["ms-auto text-ui-xs tracking-widest text-foreground-subtlest", className]);
}
