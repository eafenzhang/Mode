import { cn } from "@/components/lib/utils.js";

// P1.7 引导流程的类名契约（plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.7）。

export type OnboardingStepState = "completed" | "active" | "pending";

export function onboardingContainerClassName(className?: string): string {
  return cn([
    "flex min-h-0 flex-1 flex-col items-center justify-center gap-6 px-6 py-8",
    className,
  ]);
}

export function onboardingCardClassName(className?: string): string {
  return cn(["w-full max-w-lg rounded-xl border border-card-border bg-card p-6", className]);
}

export function onboardingTitleClassName(className?: string): string {
  return cn(["text-ui-xl font-medium text-foreground", className]);
}

export function onboardingDescriptionClassName(className?: string): string {
  return cn(["text-ui-sm text-foreground-subtle", className]);
}

export function onboardingActionsClassName(className?: string): string {
  return cn(["flex items-center justify-end gap-2 pt-2", className]);
}

const STEP_STATE_CLASSES: Record<OnboardingStepState, string> = {
  completed: "bg-success",
  active: "bg-primary",
  pending: "bg-foreground-subtlest",
};

export function onboardingStepDotClassName(state: OnboardingStepState, className?: string): string {
  return cn(["size-2 rounded-full", STEP_STATE_CLASSES[state], className]);
}

export function onboardingStepLabelClassName(className?: string): string {
  return cn(["text-ui-xs text-foreground-subtlest", className]);
}
