import type { ModeSessionFile, ModeTaskMeta } from "@mode/shared";
import { modeSessionFileSchema, modeTaskMetaSchema, modeTaskModeSchema } from "@mode/shared";

export type LegacyTaskSessionFile = Omit<ModeSessionFile, "meta"> & {
  meta: Omit<ModeTaskMeta, "mode"> & { mode?: ModeTaskMeta["mode"] };
};

const legacyTaskSessionFileSchema = modeSessionFileSchema.extend({
  // Claude 原生迁移会按清洗路径删除 meta.mode。
  // legacy snapshot 读取/写入仍要校验其它必需字段，但不能再强制把被过滤字段补回文件。
  meta: modeTaskMetaSchema.extend({
    mode: modeTaskModeSchema.optional(),
  }),
});

export function parseLegacyTaskSessionFile(input: unknown): LegacyTaskSessionFile {
  return legacyTaskSessionFileSchema.parse(input);
}

export function safeParseLegacyTaskSessionFile(input: unknown) {
  return legacyTaskSessionFileSchema.safeParse(input);
}
