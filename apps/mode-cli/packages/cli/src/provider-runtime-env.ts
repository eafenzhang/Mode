import { existsSync, realpathSync } from "node:fs";
import { readExternalEnvVar } from "@mode/shared";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
// 去智谱化只摘掉官方 CDN 远端源与数据目录缓存物化（套餐模板的投递/复活通道），
// 下面这些符号仍在用：SEA 打包路径要把随包配置物化进数据目录，三个 env 名是
// 对外契约的键值，refresh reporter 的事件类型来自 provider-node。
import {
  materializeModeBuiltinProviderConfig,
  PERSONAL_PROVIDER_CONFIG_FILE_NAME,
  MODE_BUILTIN_PROVIDER_BUNDLED_CONFIG_FILE_ENV,
  MODE_BUILTIN_PROVIDER_CONFIG_FILE_ENV,
  MODE_PERSONAL_PROVIDER_CONFIG_FILE_ENV,
  type ModeBuiltinRefreshEvent,
} from "@mode/provider-node";
import type { CliEnv } from "./env.js";

export const SEA_MODE_BUILTIN_PROVIDER_CONFIG_ASSET_KEY = "mode-provider/mode-builtin.json";

export function createCliProviderRefreshReporter(
  stderr: Pick<NodeJS.WriteStream, "write"> = process.stderr,
) {
  return {
    onBuiltinRefreshError(error: unknown) {
      stderr.write(
        `Mode Built-in 刷新失败: ${error instanceof Error ? error.message : "unknown error"}\n`,
      );
    },
    onBuiltinRefreshResult(event: ModeBuiltinRefreshEvent) {
      // TTL 检查不是生产事件；成功更新才默认留痕，不能输出 CDN URL 查询参数或内容。
      if (event.result === "updated" || process.env.NODE_ENV !== "production") {
        stderr.write(
          `Mode Built-in ${event.result}${event.reason ? ` (${event.reason})` : ""}${event.revision === undefined ? "" : ` revision=${event.revision} source=CDN`}\n`,
        );
      }
    },
  };
}

type SeaProviderConfigAssets = Pick<typeof import("node:sea"), "getAsset" | "isSea">;

interface PrepareCliProviderRuntimeEnvOptions {
  readonly argv: readonly string[];
  readonly env: CliEnv;
  readonly dataBaseDir?: string;
  readonly entrypoint?: string;
  readonly sea?: SeaProviderConfigAssets;
  readonly appVersion?: string;
  readonly platform?: string;
}

/** 为运行 Core 或写入模型选择的 CLI Entry 定位同一 Environment 的 Provider Config。 */
export async function prepareCliProviderRuntimeEnv(
  options: PrepareCliProviderRuntimeEnvOptions,
): Promise<Record<string, string>> {
  if (!requiresProviderRuntime(options.argv)) return {};

  const explicitModeBuiltin = options.env[MODE_BUILTIN_PROVIDER_CONFIG_FILE_ENV]?.trim();
  const explicitPersonal = options.env[MODE_PERSONAL_PROVIDER_CONFIG_FILE_ENV]?.trim();
  const dataBaseDir =
    options.dataBaseDir ?? readExternalEnvVar(options.env, "MODE_DATA_BASE_DIR") ?? homedir();
  if (explicitModeBuiltin && explicitPersonal) {
    return {
      [MODE_BUILTIN_PROVIDER_CONFIG_FILE_ENV]: explicitModeBuiltin,
      [MODE_PERSONAL_PROVIDER_CONFIG_FILE_ENV]: explicitPersonal,
    };
  }

  const modeBuiltinFilePath =
    explicitModeBuiltin ??
    (await resolveBundledModeBuiltinProviderConfig({
      dataBaseDir,
      entrypoint: options.entrypoint ?? process.argv[1],
      sea: options.sea ?? getSeaProviderConfigAssets(),
    }));
  const personalFilePath =
    explicitPersonal ?? join(dataBaseDir, ".zcodium", "v2", PERSONAL_PROVIDER_CONFIG_FILE_NAME);
  // Mode 去智谱化：停用官方 CDN builtin 源后，数据目录缓存（曾承载远端下发
  // 的套餐模板）不再参与；builtin 配置唯一事实源是 bundled 仓库文件，由上游
  // 同步人工维护。
  return {
    [MODE_BUILTIN_PROVIDER_CONFIG_FILE_ENV]: modeBuiltinFilePath,
    [MODE_BUILTIN_PROVIDER_BUNDLED_CONFIG_FILE_ENV]: modeBuiltinFilePath,
    [MODE_PERSONAL_PROVIDER_CONFIG_FILE_ENV]: personalFilePath,
  };
}

function requiresProviderRuntime(argv: readonly string[]): boolean {
  if (argv.some((arg) => arg === "--help" || arg === "-h" || arg === "--version" || arg === "-v")) {
    return false;
  }
  if (
    argv.some(
      (arg) =>
        arg === "--prompt" ||
        arg.startsWith("--prompt=") ||
        arg === "--target" ||
        arg.startsWith("--target="),
    )
  ) {
    return true;
  }

  const command = argv[0];
  if (command === undefined || command.startsWith("-")) return true;
  return (
    command === "tui" ||
    command === "app-server" ||
    command === "agent-server" ||
    command === "login" ||
    command === "logout"
  );
}

async function resolveBundledModeBuiltinProviderConfig(input: {
  readonly dataBaseDir: string;
  readonly entrypoint: string | undefined;
  readonly sea: SeaProviderConfigAssets | undefined;
}): Promise<string> {
  if (input.sea?.isSea()) {
    const content = input.sea.getAsset(SEA_MODE_BUILTIN_PROVIDER_CONFIG_ASSET_KEY, "utf8");
    return materializeModeBuiltinProviderConfig({
      environmentConfigRoot: join(input.dataBaseDir, ".zcodium", "v2"),
      content,
    });
  }

  const entrypoint = input.entrypoint?.trim();
  if (!entrypoint) throw new Error("无法定位 CLI Mode Built-in Provider Config：缺少入口路径");
  // 全局 bin 可以是软链接，随包配置必须相对真实入口定位。
  const entryDirectory = dirname(realpathSync(resolve(entrypoint)));
  const candidates = [
    join(entryDirectory, "provider", "mode-builtin.json"),
    resolve(entryDirectory, "../../../../../config/provider/mode-builtin.json"),
  ];
  const candidate = candidates.find((filePath) => existsSync(filePath));
  if (candidate) return candidate;
  throw new Error(`无法定位 CLI Mode Built-in Provider Config：${candidates.join(", ")}`);
}

function getSeaProviderConfigAssets(): SeaProviderConfigAssets | undefined {
  const getBuiltinModule = process.getBuiltinModule as
    | ((id: "node:sea") => typeof import("node:sea"))
    | undefined;
  return getBuiltinModule?.("node:sea");
}
