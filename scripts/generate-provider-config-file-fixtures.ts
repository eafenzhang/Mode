/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明 Provider Config 文件编解码矩阵。 */
/**
 * 生成 `provider-config-file-codec` 的 TS 对拍数据。
 *
 * 真源：`packages/provider-node/src/provider-config-file-codec.ts`。
 *
 * ```bash
 * pnpm provider-config-file:fixtures
 * cargo test -p provider-config-file-codec
 * ```
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  decodeProviderConfigFile,
  encodeProviderConfigFile,
  UnsupportedProviderConfigVersionError,
} from "../packages/provider-node/src/provider-config-file-codec.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(
  ROOT_DIR,
  "crates",
  "provider-config-file-codec",
  "fixtures",
  "files.json",
);

const providerRules = {
  providerRules: [
    {
      providerId: "custom-a",
      enabled: false,
      providerName: "Custom A",
      config: {
        group: "standard-personal",
        access: { type: "api-key", apiKey: "sk-test" },
        api: { type: "anthropic-messages", baseUrl: "https://example.com" },
        personalModelIds: ["m1", "m2"],
        modelOrder: ["m1", "m2"],
      },
    },
  ],
};

const providerModelRules = {
  providerModelRules: [
    { modelId: "m1", config: { properties: { contextWindow: 64_000 } }, providerId: "custom-a" },
  ],
  manualProviderModelRules: [
    {
      providerId: "custom-a",
      modelId: "m2",
      config: {
        enabled: true,
        properties: {
          contextWindow: 32_000,
          supportsJsonSchemaOutput: true,
          supportsNativeWebSearch: false,
          supportsMidConversationSystem: true,
          inputFormat: { supportsImage: true, supportsVideo: false, supportsPdf: true },
        },
        optionSpecs: {
          reasoningLevel: { values: ["low", "high"], map: '{"effort": reasoningLevel}' },
          maxOutputTokens: { max: 8192 },
        },
      },
    },
  ],
};

const legacyEditableManualRules = {
  providerModelRules: [],
  manualProviderModelRules: [
    {
      providerId: "custom-a",
      modelId: "m2",
      config: {
        enabled: true,
        properties: {
          requiresMfjsToolSchema: true,
          contextWindow: 32_000,
          supportsJsonSchemaOutput: true,
          supportsNativeWebSearch: false,
          supportsMidConversationSystem: true,
          inputFormat: { supportsImage: true, supportsVideo: false, supportsPdf: true },
        },
        optionSpecs: {
          reasoningLevel: { values: ["low"], map: '{"effort": reasoningLevel}' },
          maxOutputTokens: { max: 4096 },
        },
      },
    },
  ],
};

const legacyCompleteManualRules = {
  providerModelRules: [],
  manualProviderModelRules: [
    {
      providerId: "custom-a",
      modelId: "m2",
      config: {
        enabled: true,
        properties: {
          requiresMfjsToolSchema: false,
          contextWindow: 128_000,
          inputFormat: {
            supportsText: true,
            supportsImage: true,
            supportsVideo: false,
            supportsAudio: false,
            supportsPdf: true,
          },
          outputFormat: { supportsText: true },
          supportsToolCall: true,
          supportsJsonSchemaOutput: true,
          supportsNativeWebSearch: false,
          supportsMidConversationSystem: true,
        },
        optionSpecs: {
          reasoningLevel: { values: ["low", "medium"], map: '{"effort": reasoningLevel}' },
          maxOutputTokens: { max: 4096, map: '{"tokens": maxOutputTokens}' },
        },
      },
    },
  ],
};

const file = (
  modelConfigRules: unknown,
  extras: Record<string, unknown> = {},
): Record<string, unknown> => ({
  schemaVersion: 1,
  config: {
    providerOrder: ["custom-a"],
    providerConfigRules: providerRules,
    modelConfigRules,
    defaultModelSelection: {
      providerId: "custom-a",
      modelId: "m1",
      options: { reasoningLevel: "high" },
    },
    ...extras,
  },
});

const cases = [
  { name: "current-schema", input: file(providerModelRules) },
  {
    name: "current-schema-minimal",
    input: {
      schemaVersion: 1,
      config: {
        providerConfigRules: { providerRules: [] },
        modelConfigRules: { providerModelRules: [], manualProviderModelRules: [] },
      },
    },
  },
  { name: "legacy-editable-manual", input: file(legacyEditableManualRules) },
  { name: "legacy-complete-manual", input: file(legacyCompleteManualRules) },
  { name: "missing-version", input: { config: {} } },
  { name: "negative-version", input: { schemaVersion: -1, config: {} } },
  { name: "fractional-version", input: { schemaVersion: 1.5, config: {} } },
  { name: "string-version", input: { schemaVersion: "1", config: {} } },
  { name: "future-version", input: { schemaVersion: 2, config: {} } },
  { name: "zero-version", input: { schemaVersion: 0, config: {} } },
  { name: "unknown-top-level", input: { ...file(providerModelRules), extra: true } },
  { name: "unknown-config-field", input: { ...file(providerModelRules), config: { ...file(providerModelRules).config, extra: true } } },
  { name: "missing-model-rules", input: { schemaVersion: 1, config: { providerConfigRules: providerRules } } },
  { name: "empty-provider-order-item", input: { ...file(providerModelRules), config: { ...file(providerModelRules).config, providerOrder: [""] } } },
  {
    name: "legacy-unknown-key",
    input: file({
      ...legacyEditableManualRules,
      manualProviderModelRules: [
        {
          ...legacyEditableManualRules.manualProviderModelRules[0],
          config: {
            ...legacyEditableManualRules.manualProviderModelRules[0]!.config,
            unknown: true,
          },
        },
      ],
    }),
  },
  {
    name: "legacy-invalid-map",
    input: file({
      ...legacyEditableManualRules,
      manualProviderModelRules: [
        {
          ...legacyEditableManualRules.manualProviderModelRules[0],
          config: {
            ...legacyEditableManualRules.manualProviderModelRules[0]!.config,
            optionSpecs: {
              ...legacyEditableManualRules.manualProviderModelRules[0]!.config.optionSpecs,
              reasoningLevel: { values: ["low"], map: "not a map" },
            },
          },
        },
      ],
    }),
  },
];

function serializeError(error: unknown): Record<string, unknown> {
  if (error instanceof UnsupportedProviderConfigVersionError) {
    return {
      ok: false,
      kind: "version",
      message: error.message,
      version: error.version,
    };
  }
  return { ok: false, kind: "invalid" };
}

const fixture = cases.map((testCase) => {
  try {
    const decoded = decodeProviderConfigFile(testCase.input);
    return {
      name: testCase.name,
      input: testCase.input,
      expected: { ok: true, encoded: encodeProviderConfigFile(decoded) },
    };
  } catch (error) {
    return {
      name: testCase.name,
      input: testCase.input,
      expected: serializeError(error),
    };
  }
});

mkdirSync(path.dirname(OUTPUT), { recursive: true });
writeFileSync(OUTPUT, `${JSON.stringify({ cases: fixture }, null, 2)}\n`, "utf8");
process.stdout.write(`wrote ${OUTPUT}\n`);
