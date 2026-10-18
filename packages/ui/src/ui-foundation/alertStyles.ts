// P1.1 基础件补漏：Alert 的类名契约（adapter map §1 缺口）。

export type AlertTone = "neutral" | "success" | "warning" | "destructive";

const ALERT_TONE_CLASSES: Record<AlertTone, string> = {
  neutral: "border-border bg-surface text-foreground",
  success: "border-border bg-surface text-success",
  warning: "border-border bg-surface text-warning",
  destructive: "border-destructive bg-surface text-destructive",
};

export interface AlertClassNameOptions {
  tone?: AlertTone;
  className?: string;
}

export function alertClassName(options: AlertClassNameOptions = {}): string {
  const tone = options.tone ?? "neutral";
  return [
    "flex items-start gap-3 rounded-xl border p-3",
    ALERT_TONE_CLASSES[tone],
    options.className,
  ]
    .filter(Boolean)
    .join(" ");
}

export function alertTitleClassName(className?: string): string {
  return ["text-ui-base font-medium", className].filter(Boolean).join(" ");
}

export function alertDescriptionClassName(className?: string): string {
  return ["text-ui-sm text-foreground-subtle", className].filter(Boolean).join(" ");
}

export function alertActionClassName(className?: string): string {
  return ["ms-auto shrink-0", className].filter(Boolean).join(" ");
}
