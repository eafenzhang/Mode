// ============================================================
// HandoffMessage - 把消息交给同一机器人的另一个对话
// ============================================================

import { z } from "zod";
import { toToolJsonSchema } from "./json-schema.js";

export const HANDOFF_MESSAGE_TOOL_NAME = "HandoffMessage";
export const HANDOFF_MESSAGE_MAX_CONTEXT_CHARS = 4_000;
export const HANDOFF_MESSAGE_MAX_MESSAGE_CHARS = 8_000;
export const HANDOFF_MESSAGE_MAX_ATTACHMENTS = 10;

export const HandoffMessageInputSchema = z
  .object({
    target: z
      .string()
      .trim()
      .min(1)
      .describe(
        "Target conversation of the SAME bot, exactly as listed in your conversation context (e.g. 'private:u123' or 'group:oc_456').",
      ),
    message: z
      .string()
      .trim()
      .min(1)
      .max(HANDOFF_MESSAGE_MAX_MESSAGE_CHARS)
      .describe("The message for the target chat to process."),
    context: z
      .string()
      .trim()
      .max(HANDOFF_MESSAGE_MAX_CONTEXT_CHARS)
      .optional()
      .describe("Optional background so the target session can handle the handoff with its own context."),
    attachments: z
      .array(z.string().trim().min(1))
      .max(HANDOFF_MESSAGE_MAX_ATTACHMENTS)
      .optional()
      .describe("Optional file paths in the current workspace to deliver to the target chat."),
  })
  .strict();

export type HandoffMessageInput = z.infer<typeof HandoffMessageInputSchema>;
export const HandoffMessageInputJsonSchema = toToolJsonSchema(HandoffMessageInputSchema);

export const HandoffMessageOutputSchema = z
  .object({
    status: z.enum(["submitted", "queued", "duplicate"]),
    target: z.string().min(1),
  })
  .strict();

export type HandoffMessageOutput = z.infer<typeof HandoffMessageOutputSchema>;
export const HandoffMessageOutputJsonSchema = toToolJsonSchema(HandoffMessageOutputSchema);
