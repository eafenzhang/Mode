// P1.6 远程与开发者面板静态结构（plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.6）。

import { forwardRef, type ButtonHTMLAttributes, type HTMLAttributes, type ReactNode } from "react";

import {
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
} from "./remoteStyles.js";

export interface RemoteSurfaceProps extends HTMLAttributes<HTMLDivElement> {
  toolbar?: ReactNode;
  status?: ReactNode;
  offline?: boolean;
  /** 断流提示（i18n 由调用方提供）。 */
  offlineLabel?: string;
}

export const RemoteSurface = forwardRef<HTMLDivElement, RemoteSurfaceProps>(function RemoteSurface(
  { toolbar, status, offline, offlineLabel, className, children, ...rest },
  ref,
) {
  return (
    <div {...rest} ref={ref} className={remoteLayoutClassName(className)}>
      {toolbar ? <div className={remoteToolbarClassName()}>{toolbar}</div> : null}
      <div className={remoteSurfaceClassName()}>
        <div className={remoteCanvasClassName()}>{children}</div>
        {offline ? (
          <div role="status" className={remoteOfflineOverlayClassName()}>
            {offlineLabel}
          </div>
        ) : null}
      </div>
      {status ? <div className={remoteStatusStripClassName()}>{status}</div> : null}
    </div>
  );
});

export interface RemoteToolbarButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  /** 图标按钮的可访问名称。 */
  label: string;
  active?: boolean;
}

export const RemoteToolbarButton = forwardRef<HTMLButtonElement, RemoteToolbarButtonProps>(
  function RemoteToolbarButton({ label, active, className, type, children, ...rest }, ref) {
    return (
      <button
        {...rest}
        ref={ref}
        type={type ?? "button"}
        aria-label={label}
        title={label}
        aria-pressed={active}
        className={remoteToolbarButtonClassName({ active, className })}
      >
        {children}
      </button>
    );
  },
);

export interface RemoteStatusItemProps extends HTMLAttributes<HTMLSpanElement> {
  label: string;
}

export function RemoteStatusItem({
  label,
  className,
  children,
  ...rest
}: RemoteStatusItemProps): ReactNode {
  return (
    <span {...rest} className={remoteStatusItemClassName(className)}>
      <span className="text-foreground-subtlest">{label}</span>
      {children}
    </span>
  );
}

export interface TerminalPaneProps extends HTMLAttributes<HTMLDivElement> {
  label: string;
}

export const TerminalPane = forwardRef<HTMLDivElement, TerminalPaneProps>(function TerminalPane(
  { label, className, ...rest },
  ref,
) {
  return (
    <div
      {...rest}
      ref={ref}
      role="log"
      aria-label={label}
      className={terminalPaneClassName(className)}
    />
  );
});

export interface PreviewPaneProps extends HTMLAttributes<HTMLElement> {
  header?: ReactNode;
}

export const PreviewPane = forwardRef<HTMLElement, PreviewPaneProps>(function PreviewPane(
  { header, className, children, ...rest },
  ref,
) {
  return (
    <section {...rest} ref={ref} className={previewPaneClassName(className)}>
      {header ? <div className={previewHeaderClassName()}>{header}</div> : null}
      {children}
    </section>
  );
});
