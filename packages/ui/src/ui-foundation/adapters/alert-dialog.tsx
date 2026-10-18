// M4 adapter：AlertDialog（复用 Dialog 组合；Action/Cancel 关闭后收口）。

import { forwardRef, type ButtonHTMLAttributes, type ComponentProps } from "react";

import { Button } from "../Button.js";
import type { ButtonSize, ButtonVariant } from "../buttonStyles.js";

const BUTTON_VARIANTS: ButtonVariant[] = [
  "primary",
  "outline",
  "secondary",
  "ghost",
  "destructive",
  "link",
];
const BUTTON_SIZES: ButtonSize[] = [
  "xs",
  "sm",
  "default",
  "lg",
  "icon",
  "icon-xs",
  "icon-sm",
  "icon-lg",
];
function mapVariant(value: string | undefined, fallback: ButtonVariant): ButtonVariant {
  return value && (BUTTON_VARIANTS as string[]).includes(value)
    ? (value as ButtonVariant)
    : fallback;
}
function mapSize(value: string | undefined): ButtonSize {
  return value && (BUTTON_SIZES as string[]).includes(value) ? (value as ButtonSize) : "default";
}
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogPortal,
  DialogTitle,
  DialogTrigger,
} from "./dialog.js";

export {
  Dialog as AlertDialog,
  DialogContent as AlertDialogContent,
  DialogDescription as AlertDialogDescription,
  DialogFooter as AlertDialogFooter,
  DialogHeader as AlertDialogHeader,
  DialogPortal as AlertDialogPortal,
  DialogTitle as AlertDialogTitle,
  DialogTrigger as AlertDialogTrigger,
};

export type AlertDialogProps = ComponentProps<typeof Dialog>;
export type AlertDialogContentProps = ComponentProps<typeof DialogContent>;
export type AlertDialogTriggerProps = ComponentProps<typeof DialogTrigger>;

export interface AlertDialogActionProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  asChild?: boolean;
  variant?: string;
  size?: string;
}

export const AlertDialogAction = forwardRef<HTMLButtonElement, AlertDialogActionProps>(
  function AlertDialogAction({ onClick, asChild: _asChild, variant, size, ...rest }, ref) {
    return (
      <DialogClose>
        <Button
          {...rest}
          ref={ref}
          variant={mapVariant(variant, "primary")}
          size={mapSize(size)}
          onClick={(event) => {
            onClick?.(event);
          }}
        />
      </DialogClose>
    );
  },
);

export interface AlertDialogCancelProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  asChild?: boolean;
  variant?: string;
  size?: string;
}

export const AlertDialogCancel = forwardRef<HTMLButtonElement, AlertDialogCancelProps>(
  function AlertDialogCancel({ onClick, asChild: _asChild, variant, size, ...rest }, ref) {
    return (
      <DialogClose>
        <Button
          {...rest}
          ref={ref}
          variant={mapVariant(variant, "outline")}
          size={mapSize(size)}
          onClick={(event) => {
            onClick?.(event);
          }}
        />
      </DialogClose>
    );
  },
);

/** 旧 API 的遮罩：foundation Dialog 不提供独立遮罩层，保留空实现避免调用方报错。 */
export function AlertDialogOverlay(): null {
  return null;
}
