import { z } from "zod";

/**
 * Mode agent 提供方的单一真源。
 *
 * 类型 ModeProvider、运行时 schema modeProviderSchema 都从这里派生,
 * 避免各处内联 z.enum([...]) 副本随新增/删除 provider 漂移。
 * 本模块只依赖 zod(叶子),可被 validation / mode-protocol 等无环引用。
 */
const MODE_PROVIDERS = ["glm"] as const;

export const modeProviderSchema = z.enum(MODE_PROVIDERS);

export type ModeProvider = (typeof MODE_PROVIDERS)[number];
