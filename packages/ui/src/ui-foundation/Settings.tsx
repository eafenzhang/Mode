// P1.5 设置页与插件卡片静态结构（plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.5）。

import {
  forwardRef,
  type ButtonHTMLAttributes,
  type HTMLAttributes,
  type ReactNode,
} from "react";

import {
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
} from "./settingsStyles.js";

export interface SettingsLayoutProps extends HTMLAttributes<HTMLDivElement> {
  navLabel: string;
  nav: ReactNode;
}

export const SettingsLayout = forwardRef<HTMLDivElement, SettingsLayoutProps>(
  function SettingsLayout({ navLabel, nav, className, children, ...rest }, ref) {
    return (
      <div {...rest} ref={ref} className={settingsLayoutClassName(className)}>
        <nav aria-label={navLabel} className={settingsNavClassName()}>
          {nav}
        </nav>
        <div className={settingsContentClassName()}>{children}</div>
      </div>
    );
  },
);

export interface SettingsNavItemProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  selected?: boolean;
}

export const SettingsNavItem = forwardRef<HTMLButtonElement, SettingsNavItemProps>(
  function SettingsNavItem({ selected, className, type, children, ...rest }, ref) {
    return (
      <button
        {...rest}
        ref={ref}
        type={type ?? "button"}
        aria-current={selected ? "page" : undefined}
        className={settingsNavItemClassName({ selected, className })}
      >
        {children}
      </button>
    );
  },
);

export interface SettingsSectionProps extends Omit<HTMLAttributes<HTMLElement>, "title"> {
  title: ReactNode;
  description?: ReactNode;
}

export const SettingsSection = forwardRef<HTMLElement, SettingsSectionProps>(
  function SettingsSection({ title, description, className, children, ...rest }, ref) {
    return (
      <section
        {...rest}
        ref={ref}
        aria-label={typeof title === "string" ? title : undefined}
        className={settingsSectionClassName(className)}
      >
        <div className="flex flex-col gap-1">
          <div className={settingsSectionTitleClassName()}>{title}</div>
          {description ? (
            <div className={settingsSectionDescriptionClassName()}>{description}</div>
          ) : null}
        </div>
        {children}
      </section>
    );
  },
);

export interface PluginCardProps extends Omit<HTMLAttributes<HTMLElement>, "title"> {
  title: ReactNode;
  description?: ReactNode;
  icon?: ReactNode;
  /** 无值表示可安装；两类值只改变 badge 与安装入口。 */
  availability?: PluginAvailability;
  badgeLabel?: string;
  actions?: ReactNode;
}

export const PluginCard = forwardRef<HTMLElement, PluginCardProps>(function PluginCard(
  { title, description, icon, availability, badgeLabel, actions, className, ...rest },
  ref,
) {
  return (
    <section
      {...rest}
      ref={ref}
      data-availability={availability}
      aria-label={typeof title === "string" ? title : undefined}
      className={pluginCardClassName(className)}
    >
      <div className={pluginCardHeaderClassName()}>
        {icon ? <div className={pluginIconClassName()}>{icon}</div> : null}
        <div className={pluginCardBodyClassName()}>
          <div className={pluginCardTitleClassName()}>{title}</div>
          {description ? (
            <div className={pluginCardDescriptionClassName()}>{description}</div>
          ) : null}
        </div>
        {availability && badgeLabel ? (
          <span className={pluginAvailabilityBadgeClassName()}>{badgeLabel}</span>
        ) : null}
        {actions ? <div className={pluginCardActionsClassName()}>{actions}</div> : null}
      </div>
    </section>
  );
});

export interface PluginUnavailableHintProps extends HTMLAttributes<HTMLDivElement> {
  children?: ReactNode;
}

export const PluginUnavailableHint = forwardRef<HTMLDivElement, PluginUnavailableHintProps>(
  function PluginUnavailableHint({ className, children, ...rest }, ref) {
    return (
      <div
        {...rest}
        ref={ref}
        role="note"
        className={pluginDetailHintClassName(className)}
      >
        {children}
      </div>
    );
  },
);
