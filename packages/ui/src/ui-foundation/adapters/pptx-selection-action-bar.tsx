// M4 adapter：PptxSelectionActionBar（旧 API：选区操作条）。
// 幻灯片内容保持原位，操作条 portal 到 body 并按 boundary 夹取水平位置。

import {
  useEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";

import { Button } from "../Button.js";
import { Textarea } from "../Textarea.js";
import { isImeComposingKeyEvent } from "@/lib/imeComposition.js";
import {
  normalizePptxComment,
  resolvePptxActionBarPosition,
  shouldSubmitPptxComment,
  type PptxActionBarPosition,
} from "../pptxSelectionState.js";

export interface PptxSelectionActionBarProps {
  children: ReactNode;
  open: boolean;
  boundary: HTMLElement | null;
  label: string;
  commentPlaceholder: string;
  cancelLabel: string;
  addToConversationLabel: string;
  editing: boolean;
  disabled: boolean;
  onAiEdit: () => void;
  onCancelAiEdit: () => void;
  onAddToConversation: (comment: string) => void;
  onExitSelection: () => void;
}

export function PptxSelectionActionBar({
  children,
  open,
  boundary,
  label,
  commentPlaceholder,
  cancelLabel,
  addToConversationLabel,
  editing,
  disabled,
  onAiEdit,
  onCancelAiEdit,
  onAddToConversation,
  onExitSelection,
}: PptxSelectionActionBarProps): ReactNode {
  const [position, setPosition] = useState<PptxActionBarPosition | null>(null);
  const [comment, setComment] = useState("");
  const compositionRef = useRef(false);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);

  useEffect(() => {
    if (!open || !boundary || typeof window === "undefined") {
      setPosition(null);
      return;
    }
    const update = (): void => {
      setPosition(
        resolvePptxActionBarPosition(boundary.getBoundingClientRect(), window.innerWidth),
      );
    };
    update();
    window.addEventListener("resize", update);
    window.addEventListener("scroll", update, true);
    return () => {
      window.removeEventListener("resize", update);
      window.removeEventListener("scroll", update, true);
    };
  }, [open, boundary]);

  useEffect(() => {
    if (!open) {
      setComment("");
      return;
    }
    if (editing) {
      textareaRef.current?.focus();
    }
  }, [open, editing]);

  useEffect(() => {
    if (!open || typeof document === "undefined") {
      return;
    }
    const handleKey = (event: KeyboardEvent): void => {
      if (
        event.key !== "Escape" ||
        isImeComposingKeyEvent({
          compositionActive: compositionRef.current,
          isComposing: event.isComposing,
        })
      ) {
        return;
      }
      event.preventDefault();
      if (editing) {
        onCancelAiEdit();
      } else {
        onExitSelection();
      }
    };
    document.addEventListener("keydown", handleKey);
    return () => document.removeEventListener("keydown", handleKey);
  }, [open, editing, onCancelAiEdit, onExitSelection]);

  const submit = (): void => {
    if (!shouldSubmitPptxComment(comment, disabled)) {
      return;
    }
    onAddToConversation(normalizePptxComment(comment));
    setComment("");
  };

  const handleTextareaKeyDown = (event: ReactKeyboardEvent<HTMLTextAreaElement>): void => {
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      submit();
    }
  };

  const bar =
    open && position ? (
      <div
        role="toolbar"
        aria-label={label}
        data-slot="pptx-selection-action-bar"
        style={{
          position: "fixed",
          left: position.left,
          top: position.top,
          transform: "translateX(-50%)",
          zIndex: 60,
        }}
        className="flex max-w-[calc(100vw-2rem)] items-start gap-2 rounded-xl border border-popover-border bg-popover p-2 shadow-lg"
        onCompositionStart={() => {
          compositionRef.current = true;
        }}
        onCompositionEnd={() => {
          compositionRef.current = false;
        }}
      >
        {editing ? (
          <div className="flex w-72 flex-col gap-2">
            <Textarea
              ref={textareaRef}
              value={comment}
              placeholder={commentPlaceholder}
              rows={3}
              onChange={(event) => setComment(event.target.value)}
              onKeyDown={handleTextareaKeyDown}
              disabled={disabled}
            />
            <div className="flex justify-end gap-2">
              <Button size="sm" variant="ghost" onClick={onCancelAiEdit} disabled={disabled}>
                {cancelLabel}
              </Button>
              <Button size="sm" onClick={submit} disabled={disabled}>
                {addToConversationLabel}
              </Button>
            </div>
          </div>
        ) : (
          <>
            <Button size="sm" onClick={onAiEdit} disabled={disabled}>
              {label}
            </Button>
            <Button
              size="icon-sm"
              variant="ghost"
              aria-label={cancelLabel}
              onClick={onExitSelection}
              disabled={disabled}
            >
              <svg
                aria-hidden="true"
                viewBox="0 0 16 16"
                className="size-3.5"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.5"
                strokeLinecap="round"
              >
                <path d="M4 4l8 8M12 4l-8 8" />
              </svg>
            </Button>
          </>
        )}
      </div>
    ) : null;

  return (
    <>
      {children}
      {bar && typeof document !== "undefined" && document.body
        ? createPortal(bar, document.body)
        : null}
    </>
  );
}

export type { PptxActionBarPosition };
