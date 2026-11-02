/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明旧 config.json→Provider 读取矩阵，拆分会降低可核对性。 */
/**
 * 生成 `provider-legacy-migration` 的旧 config.json 读取对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/model-provider/legacyModeConfigProviderReader.ts`
 *
 * 每条用例写入临时 data root 的 `v2/config.json`，再调用公开的
 * `readLegacyModeConfigProviders()`；结果里 undefined 字段由 JSON 序列化省略。
 *
 * ```bash
 * pnpm provider-legacy-reader:fixtures
 * cargo test -p provider-legacy-migration
 * ```
 */
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { resolveBigModelApiOrigin, resolveRuntimeModeEnv } from "../packages/shared/src/modeEndpoint.js";
import { readLegacyModeConfigProviders } from "../packages/services/src/model-provider/legacyModeConfigProviderReader.js";
import { setDataRootPathOverride } from "../packages/services/src/paths.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(
  ROOT_DIR,
  "crates",
  "provider-legacy-migration",
  "fixtures",
  "legacy-config-reader.json",
);
const NOW = 1_700_000_000_000;

interface FixtureCase {
  name: string;
  env: { runtimeEnv: string; bigModelApiOrigin: string };
  config: Record<string, unknown>;
}

function config(provider: Record<string, unknown> | undefined): Record<string, unknown> {
  return {
    $schema: "https://example.invalid/mode-config.schema.json",
    ...(provider === undefined ? {} : { provider }),
  };
}

const cases: FixtureCase[] = [
  {
    name: "open-code-fields-win-over-legacy-extension",
    env: { runtimeEnv: "production", bigModelApiOrigin: "https://bigmodel.cn" },
    config: config({
      "custom-a": {
        kind: "anthropic",
        id: " custom-a ",
        name: " Custom A ",
        options: {
          baseURL: " https://API.Example.com/v1/ ",
          apiKey: " sk-test-a ",
          headers: { " X-Token ": " abc ", empty: "   " },
        },
        api: "https://ignored.example.com",
        enabled: false,
        systemDisabledReason: "oauth_provider_inactive",
        apiKeyRequired: true,
        logoUrl: "https://example.com/logo.svg",
        apiKeyUrl: "https://example.com/key",
        providerMappings: { claude: { source: "x" }, "mode-a": { source: "m" } },
        createdAt: 111,
        updatedAt: 222,
        models: {
          "claude-x": {
            id: "claude-x",
            name: " Claude X ",
            limit: { context: 123_456.9, output: 8_192.4 },
            modalities: {
              input: ["text", "image", "text"],
              output: ["text", "pdf"],
            },
            reasoning: {
              enabled: true,
              variants: ["low", "high", "low"],
              defaultVariant: "high",
            },
            priority: 3,
            supportsTools: true,
            customField: { ignored: true },
          },
          "claude-y": {
            limit: { context: 512 },
            options: { max_tokens: 4_096 },
            hasMaxOutputTokens: true,
            deleted: true,
            name: "   ",
          },
        },
        mode: {
          enabled: true,
          systemDisabledReason: "coding_plan_not_connected",
          endpoints: { baseURL: "https://legacy.example.com", paths: { openai: "/v1" } },
          apiFormat: "openai-chat-completions",
          source: "builtin",
          apiKeyRequired: false,
          headers: { "X-Mode": "1" },
          createdAt: 1,
          updatedAt: 2,
          deletedModels: ["claude-y", "gone-model", "gone-model"],
        },
      },
    }),
  },
  {
    name: "legacy-mode-extension-fallbacks",
    env: { runtimeEnv: "production", bigModelApiOrigin: "https://bigmodel.cn" },
    config: config({
      "legacy-a": {
        id: "legacy-a",
        name: "Legacy A",
        options: { apiKey: "sk-legacy", headers: { " X-Legacy ": " yes " } },
        createdAt: 1000,
        models: {
          alpha: {
            name: " Alpha ",
            contextWindow: 64_000.8,
            kinds: ["anthropic", "anthropic", "openai"],
            defaultKind: "anthropic",
            modalities: { input: ["text", "text"], output: ["image", "image"] },
            reasoningSpec: { levels: { low: {}, high: {} }, defaultLevel: "low" },
            priority: 1.5,
            disabledReason: "temporarily disabled",
            supportsStructuredOutput: false,
            modified: true,
          },
          beta: {
            kinds: ["openai"],
            mode: {
              kinds: ["openai-compatible", "openai"],
              defaultKind: "openai-compatible",
              reasoning: {
                levels: {
                  medium: { anthropic: { set: [{ path: ["x"], value: 1 }] } },
                },
              },
              priority: 2,
              disabledReason: "mode disabled",
              supportsTools: false,
              modified: true,
              deleted: false,
            },
          },
          gamma: {
            reasoning: { variants: ["a", "b"], defaultVariant: "b" },
          },
          tombstone: { deleted: true, name: "Tombstone", contextWindow: 10 },
        },
        providerMappings: {
          claude: { source: "legacy" },
          "mode-one": { source: "mode" },
          keep: { source: "keep" },
        },
        apiFormat: "anthropic-messages",
        defaultKind: "anthropic",
        endpoints: {
          baseURL: "https://legacy.example.com/base/",
          paths: { anthropic: "/v1/messages" },
        },
        source: "builtin",
        catalogSourceId: "china-llm-mode-dev",
        catalogProviderId: "provider-a",
        modelsDevProviderId: "models-dev-a",
        apiKeyRequired: false,
        headers: { " X-Provider ": " value " },
        logoUrl: "https://example.com/legacy.svg",
        apiKeyUrl: "https://example.com/legacy-key",
        mode: {
          enabled: true,
          systemDisabledReason: "coding_plan_not_authenticated",
          createdAt: 11,
          updatedAt: 22,
          deletedModels: ["ghost", "beta"],
        },
      },
    }),
  },
  {
    name: "legacy-endpoints-only",
    env: { runtimeEnv: "production", bigModelApiOrigin: "https://bigmodel.cn" },
    config: config({
      "legacy-endpoints": {
        endpoints: { anthropic: " https://anthropic.example.com/v1/ " },
        models: { first: { contextWindow: 1 } },
        createdAt: 5,
      },
      "legacy-openai": {
        endpoints: { openai: " https://openai.example.com/v1/ " },
        models: { second: { contextWindow: 2 } },
      },
      "legacy-both": {
        endpoints: {
          anthropic: "https://both.example.com/anthropic",
          openai: "https://both.example.com/openai",
        },
        models: { third: { contextWindow: 3 } },
      },
    }),
  },
  {
    name: "bigmodel-coding-plan-legacy-runtime-production",
    env: { runtimeEnv: "production", bigModelApiOrigin: "https://bigmodel.cn" },
    config: config({
      "account:bigmodel-individual-coding-plan": {
        kind: "openai-compatible",
        options: {
          baseURL: "https://open.bigmodel.cn/api/coding/paas/v4/",
          apiKey: "sk-bigmodel",
        },
        models: { "glm-5": { limit: { context: 131_072 } } },
      },
    }),
  },
  {
    name: "bigmodel-coding-plan-legacy-runtime-test",
    env: {
      runtimeEnv: "test",
      bigModelApiOrigin: "https://test-bigmodel.example.com/root/",
    },
    config: config({
      "account:bigmodel-individual-coding-plan": {
        kind: "openai-compatible",
        options: {
          baseURL: "https://open.bigmodel.cn/api/coding/paas/v4",
          apiKey: "sk-bigmodel",
        },
        models: { "glm-5": { limit: { context: 131_072 } } },
      },
    }),
  },
  {
    name: "preset-glm-runtime-kinds",
    env: { runtimeEnv: "production", bigModelApiOrigin: "https://bigmodel.cn" },
    config: config({
      "zai-api": {
        kind: "anthropic",
        options: { baseURL: "https://api.example.com/coding/paas/v4" },
        models: { "glm-4": {} },
      },
      "bigmodel-api": {
        kind: "openai-compatible",
        options: { baseURL: "https://api.example.com/api/anthropic" },
        models: { "glm-3": {} },
      },
      "account:zai-start-plan": {
        kind: "openai-compatible",
        options: { baseURL: "https://api.example.com/mode-plan/anthropic" },
        models: {},
      },
    }),
  },
  {
    name: "store-migrations-drop-retired-and-empty-defaults",
    env: { runtimeEnv: "production", bigModelApiOrigin: "https://bigmodel.cn" },
    config: config({
      "builtin:zapi": {
        id: "builtin:zapi",
        options: { apiKey: "sk-retired", baseURL: "https://retired.example.com" },
        models: { old: {} },
      },
      "default-empty": {
        id: "default-empty",
        options: { apiKey: "   ", baseURL: "https://default.example.com" },
        models: { old: {} },
      },
      "default-with-key": {
        id: "default-with-key",
        options: { apiKey: "sk-default", baseURL: "https://default.example.com" },
        models: { kept: {} },
      },
    }),
  },
  {
    name: "open-code-kind-with-canonical-endpoints",
    env: { runtimeEnv: "production", bigModelApiOrigin: "https://bigmodel.cn" },
    config: config({
      "kind-openai": {
        kind: "openai",
        endpoints: {
          baseURL: "https://responses.example.com/v1/",
          paths: { openai: "/responses" },
        },
        apiFormat: "anthropic-messages",
        defaultKind: "anthropic",
        models: {
          m: {
            kinds: ["openai", "openai-compatible"],
            defaultKind: "openai",
            contextWindow: 32_000,
            maxOutputTokens: 2_000,
            modalities: { input: ["text"], output: ["audio", "text"] },
          },
        },
      },
      "kind-openai-compatible": {
        kind: "openai-compatible",
        options: { baseURL: "https://chat.example.com/v1/messages/" },
        models: {},
      },
    }),
  },
  {
    name: "missing-provider",
    env: { runtimeEnv: "production", bigModelApiOrigin: "https://bigmodel.cn" },
    config: config(undefined),
  },
  {
    name: "empty-provider-map",
    env: { runtimeEnv: "production", bigModelApiOrigin: "https://bigmodel.cn" },
    config: config({}),
  },
];

async function main(): Promise<void> {
  const originalNow = Date.now;
  const originalEnv = {
    MODE_ENV: process.env.MODE_ENV,
    BIGMODEL_API_BASE_URL: process.env.BIGMODEL_API_BASE_URL,
  };
  const root = mkdtempSync(path.join(tmpdir(), "mode-legacy-reader-"));
  const fixtures: Array<Record<string, unknown>> = [];
  try {
    Date.now = () => NOW;
    for (const testCase of cases) {
      process.env.MODE_ENV = testCase.env.runtimeEnv;
      process.env.BIGMODEL_API_BASE_URL = testCase.env.bigModelApiOrigin;
      const dataRoot = path.join(root, testCase.name);
      mkdirSync(path.join(dataRoot, "v2"), { recursive: true });
      writeFileSync(
        path.join(dataRoot, "v2", "config.json"),
        `${JSON.stringify(testCase.config, null, 2)}\n`,
        "utf8",
      );
      setDataRootPathOverride(dataRoot);
      const providers = await readLegacyModeConfigProviders();
      fixtures.push({
        name: testCase.name,
        env: {
          runtime_env: resolveRuntimeModeEnv(process.env),
          bigmodel_api_origin: resolveBigModelApiOrigin(process.env),
        },
        config: testCase.config,
        providers,
      });
    }
  } finally {
    setDataRootPathOverride(null);
    Date.now = originalNow;
    for (const [key, value] of Object.entries(originalEnv)) {
      if (value === undefined) {
        delete process.env[key];
      } else {
        process.env[key] = value;
      }
    }
    rmSync(root, { recursive: true, force: true });
  }

  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(
    OUTPUT,
    `${JSON.stringify({ generatedAt: NOW, nowMs: NOW, cases: fixtures }, null, 2)}\n`,
    "utf8",
  );
  process.stdout.write(`wrote ${OUTPUT}\n`);
}

await main();
