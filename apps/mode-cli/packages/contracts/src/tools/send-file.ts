// ============================================================
// SendFile - 把当前工作区内的文件发到当前 IM bot 会话
// ============================================================

import { z } from "zod";
import { toToolJsonSchema } from "./json-schema.js";

export const SEND_FILE_TOOL_NAME = "SendFile";
export const SEND_FILE_MAX_CAPTION_CHARS = 2_000;

export const SendFileInputSchema = z
  .object({
    path: z
      .string()
      .trim()
      .min(1)
      .describe(
        "File path to send: relative to the current workspace (recommended) or an absolute path inside the workspace, app data directory, or system temp directory.",
      ),
    caption: z
      .string()
      .trim()
      .max(SEND_FILE_MAX_CAPTION_CHARS)
      .optional()
      .describe("Optional short note shown with the file."),
  })
  .strict();

export type SendFileInput = z.infer<typeof SendFileInputSchema>;

export const SendFileInputJsonSchema = toToolJsonSchema(SendFileInputSchema);

export const SendFileOutputSchema = z
  .object({
    status: z.literal("sent"),
    filename: z.string().min(1),
    kind: z.enum(["image", "file"]),
  })
  .strict();

export type SendFileOutput = z.infer<typeof SendFileOutputSchema>;

export const SendFileOutputJsonSchema = toToolJsonSchema(SendFileOutputSchema);
