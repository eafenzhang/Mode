// P1.7 引导流程的类名契约（plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.7）。

export type OnboardingStepState = "completed" | "active" | "pending";

export function onboardingContainerClassName(className?: string): string {
  return ["flex min-h-0 flex-1 flex-col items-center justify-center gap-6 px-6 py-8", className]
    .filter(Boolean)
    .join(" ");
}

export function onboardingCardClassName(className?: string): string {
  return ["w-full max-w-lg rounded-xl border border-card-border bg-card p-6", className]
    .filter(Boolean)
    .join(" ");
}

export function onboardingTitleClassName(className?: string): string {
  return ["text-ui-xl font-medium text-foreground", className].filter(Boolean).join(" ");
}

export function onboardingDescriptionClassName(className?: string): string {
  return ["text-ui-sm text-foreground-subtle", className].filter(Boolean).join(" ");
}

export function onboardingActionsClassName(className?: string): string {
  return ["flex items-center justify-end gap-2 pt-2", className].filter(Boolean).join(" ");
}

const STEP_STATE_CLASSES: Record<OnboardingStepState, string> = {
  completed: "bg-success",
  active: "bg-primary",
  pending: "bg-foreground-subtlest",
};

export function onboardingStepDotClassName(state: OnboardingStepState, className?: string): string {
  return ["size-2 rounded-full", STEP_STATE_CLASSES[state], className].filter(Boolean).join(" ");
}

export function onboardingStepLabelClassName(className?: string): string {
  return ["text-ui-xs text-foreground-subtlest", className].filter(Boolean).join(" ");
}
