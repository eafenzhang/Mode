// P1.7 引导流程静态结构（plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.7）。

import { forwardRef, type HTMLAttributes, type ReactNode } from "react";

import {
  onboardingActionsClassName,
  onboardingCardClassName,
  onboardingContainerClassName,
  onboardingDescriptionClassName,
  onboardingStepDotClassName,
  onboardingStepLabelClassName,
  onboardingTitleClassName,
  type OnboardingStepState,
} from "./onboardingStyles.js";

export interface OnboardingShellProps extends Omit<HTMLAttributes<HTMLDivElement>, "title"> {
  title: ReactNode;
  description?: ReactNode;
  steps?: OnboardingStepState[];
  /** 步骤文案（i18n，如「第 2 / 4 步」）。 */
  stepLabel?: string;
  actions?: ReactNode;
}

export const OnboardingShell = forwardRef<HTMLDivElement, OnboardingShellProps>(
  function OnboardingShell(
    { title, description, steps, stepLabel, actions, className, children, ...rest },
    ref,
  ) {
    return (
      <div {...rest} ref={ref} className={onboardingContainerClassName(className)}>
        <div className={onboardingCardClassName()}>
          {steps && steps.length > 0 ? (
            <div className="mb-4 flex items-center gap-1.5" aria-hidden="true">
              {steps.map((state, index) => (
                <span key={index} className={onboardingStepDotClassName(state)} />
              ))}
            </div>
          ) : null}
          {stepLabel ? <div className={onboardingStepLabelClassName()}>{stepLabel}</div> : null}
          <div className={onboardingTitleClassName()}>{title}</div>
          {description ? (
            <div className={`mt-2 ${onboardingDescriptionClassName()}`}>{description}</div>
          ) : null}
          {children ? <div className="mt-4">{children}</div> : null}
          {actions ? <div className={onboardingActionsClassName()}>{actions}</div> : null}
        </div>
      </div>
    );
  },
);
