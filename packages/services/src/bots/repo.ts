import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { atomicWritePrivateTextFile, withFileLock } from "@zcode/shared/node";
import {
  botBindingsFileSchema,
  botsConfigFileSchema,
  RETIRED_BOT_PROVIDERS,
  botsStateFileSchema,
  type BotBindingsFile,
  type BotsConfigFile,
  type BotsStateFile,
} from "@zcode/shared";
import { getAppConfigDir } from "../paths.js";
import {
  BOTS_BINDINGS_FILE,
  BOTS_CONFIG_FILE,
  BOTS_LEGACY_CONFIG_FILE,
  BOTS_LEGACY_STATE_FILE,
  BOTS_V2_STATE_FILE,
  BOTS_STATE_FILE,
  createDefaultBotsConfig,
} from "./config.js";
import { importLegacyBotConfig, importLegacyBotState } from "./storageMigration.js";

/** 统计原始配置里已被下线通道的机器人条数（解析时会被丢弃）。 */
function countRetiredBotRecords(value: unknown): number {
  const bots = (value as { bots?: unknown } | null)?.bots;
  if (!Array.isArray(bots)) {
    return 0;
  }
  return bots.filter((bot) => {
    const provider = (bot as { provider?: unknown } | null)?.provider;
    return typeof provider === "string" && RETIRED_BOT_PROVIDERS.includes(provider);
  }).length;
}

async function readOptionalJson(path: string): Promise<unknown | undefined> {
  try {
    return JSON.parse(await readFile(path, "utf8"));
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
    throw error;
  }
}

async function writeJson(path: string, value: unknown): Promise<void> {
  await atomicWritePrivateTextFile(path, `${JSON.stringify(value, null, 2)}\n`);
}

export class BotsRepo {
  async readConfig(): Promise<BotsConfigFile> {
    const path = join(getAppConfigDir(), BOTS_CONFIG_FILE);
    return withFileLock(path, async () => {
      const current = await readOptionalJson(path);
      // 回滚兼容：v3 已存在就只认 v3；损坏时暴露错误，绝不能恢复旧 Bot 或覆盖用户新修改。
      if (current !== undefined) {
        const parsed = botsConfigFileSchema.parse(current);
        // 已下线通道（AstrBot 桥接）的记录在解析阶段被丢弃；在同一把文件锁内把清理结果固定下来，
        // 否则磁盘上会长期留着再也读不到的记录。
        if (countRetiredBotRecords(current) > 0) {
          await writeJson(path, parsed);
        }
        return parsed;
      }
      const legacy = await readOptionalJson(join(getAppConfigDir(), BOTS_LEGACY_CONFIG_FILE));
      const config = botsConfigFileSchema.parse(
        legacy === undefined ? createDefaultBotsConfig() : importLegacyBotConfig(legacy),
      );
      await writeJson(path, config);
      return config;
    });
  }

  async writeConfig(config: BotsConfigFile): Promise<BotsConfigFile> {
    const parsed = botsConfigFileSchema.parse(config);
    const path = join(getAppConfigDir(), BOTS_CONFIG_FILE);
    await withFileLock(path, () => writeJson(path, parsed));
    return parsed;
  }

  async readState(): Promise<BotsStateFile> {
    const path = join(getAppConfigDir(), BOTS_STATE_FILE);
    return withFileLock(path, async () => {
      const current = await readOptionalJson(path);
      if (current !== undefined) return botsStateFileSchema.parse(current);
      const v2 = await readOptionalJson(join(getAppConfigDir(), BOTS_V2_STATE_FILE));
      const legacy =
        v2 === undefined
          ? await readOptionalJson(join(getAppConfigDir(), BOTS_LEGACY_STATE_FILE))
          : v2;
      const state = botsStateFileSchema.parse(
        legacy === undefined ? { version: 3, bots: {} } : importLegacyBotState(legacy),
      );
      // 在同一文件锁内固定迁移结果；后续登录/套餐变化不再重新解释旧身份。
      await writeJson(path, state);
      return state;
    });
  }

  /**
   * 工作区 → bot 绑定表。独立文件 + 文件锁：绑定曾被写进 setting.json，
   * 而设置文件有多个写入方（tab 持久化、settings-sync、迁移提交），
   * 任一持有过期快照的写入都会把绑定整块覆盖成 {}，导致"绑定后又变未绑定"。
   */
  async readBindings(): Promise<BotBindingsFile> {
    const path = join(getAppConfigDir(), BOTS_BINDINGS_FILE);
    return withFileLock(path, async () => {
      const current = await readOptionalJson(path);
      if (current !== undefined) {
        return botBindingsFileSchema.parse(current);
      }
      return { version: 1, bindings: {} } satisfies BotBindingsFile;
    });
  }

  async writeBindings(bindings: BotBindingsFile): Promise<BotBindingsFile> {
    const parsed = botBindingsFileSchema.parse(bindings);
    const path = join(getAppConfigDir(), BOTS_BINDINGS_FILE);
    await withFileLock(path, () => writeJson(path, parsed));
    return parsed;
  }

  async writeState(state: BotsStateFile): Promise<BotsStateFile> {
    const parsed = botsStateFileSchema.parse(state);
    const path = join(getAppConfigDir(), BOTS_STATE_FILE);
    await withFileLock(path, () => writeJson(path, parsed));
    return parsed;
  }
}
