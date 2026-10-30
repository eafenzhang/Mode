/* oxlint-disable eslint(max-lines) -- 对拍数据集中声明合法/非法样例，拆分会降低可核对性。 */
/**
 * 生成 `provider-config` 的 Rust 对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/provider/src/config/*.ts`
 * - `packages/shared/src/model-config.ts`
 *
 * ```bash
 * pnpm provider-config:fixtures
 * cmd //c "D:\\tools\\cargo-msvc.cmd test -p provider-config"
 * ```
 *
 * 每条解析记录保存：输入 JSON 原文、是否成功、成功时的 `toJSON()` 原文与解析后 JSON、
 * 失败时的 zod/构造器问题。Rust 侧用同一份输入重建，成功按原文逐字节比较，
 * 失败按接受/拒绝与路径集合比较。
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { z } from "zod";

import { manualModelConfigSchema } from "../packages/provider/src/config/manual-model-config.js";
import {
  parseAccountProviderConfigMap,
  parseModeBuiltinModelConfigRules,
  parseModeBuiltinProviderConfigRules,
  parseModelConfig,
  parsePersonalModelConfigRules,
  parsePersonalProviderConfigMap,
  parseProviderConfig,
  parseProviderConfigMap,
  parseProviderTemplateMap,
} from "../packages/provider/src/config/schema.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT_DIR, "crates", "provider-config", "fixtures", "provider-config.json");

interface SchemaIssue {
  code: string;
  path: string[];
  message: string;
}

interface ParseRecord {
  name: string;
  kind: string;
  input: string;
  ok: boolean;
  output?: string;
  outputJson?: unknown;
  issues?: SchemaIssue[];
  error?: string;
}

interface ValidationRecord {
  name: string;
  kind: string;
  input: string;
  issues: SchemaIssue[];
}

const records: ParseRecord[] = [];
const validations: ValidationRecord[] = [];

function schemaIssues(error: unknown): SchemaIssue[] {
  if (error instanceof z.ZodError) {
    return error.issues.map((issue) => ({
      code: issue.code,
      path: issue.path.map(String),
      message: issue.message,
    }));
  }
  return [];
}

function addParse(name: string, kind: string, input: unknown, run: () => unknown): void {
  const rawInput = JSON.stringify(input);
  try {
    const value = run();
    const output = JSON.stringify(value);
    records.push({ name, kind, input: rawInput, ok: true, output, outputJson: JSON.parse(output) });
  } catch (error) {
    records.push({
      name,
      kind,
      input: rawInput,
      ok: false,
      issues: schemaIssues(error),
      error: error instanceof Error ? error.message : String(error),
    });
  }
}

function addValidation(
  name: string,
  kind: string,
  input: unknown,
  run: () => unknown,
  validate: (value: any) => readonly { code: string; path: readonly (string | number)[]; message: string }[],
): void {
  const rawInput = JSON.stringify(input);
  const value = run();
  const issues = validate(value).map((issue) => ({
    code: issue.code,
    path: issue.path.map(String),
    message: issue.message,
  }));
  validations.push({ name, kind, input: rawInput, issues });
}

// ---------------------------------------------------------------------------
// 复用输入
// ---------------------------------------------------------------------------

const CEL_REASONING = '{"effort": reasoningLevel}';
const CEL_MAX_TOKENS = '{"tokens": maxOutputTokens}';
const COMPLETE_PROPERTIES = {
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
};
const MANUAL_PROPERTIES = {
  contextWindow: 64_000,
  supportsJsonSchemaOutput: true,
  supportsNativeWebSearch: false,
  supportsMidConversationSystem: true,
  inputFormat: { supportsImage: true, supportsVideo: false, supportsPdf: true },
};
const MANUAL_OPTION_SPECS = {
  reasoningLevel: { values: ["low", "high"], map: CEL_REASONING },
  maxOutputTokens: { max: 8_192 },
};

const FULL_PROVIDER_CONFIG = {
  group: "zai-family",
  logo: { type: "builtin", key: "zhipu" },
  access: { type: "api-key", apiKey: "sk-1", apiKeyManagementUrl: "https://keys.example.com" },
  api: {
    type: "openai-chat-completions",
    baseUrl: "https://api.example.com/v1",
    headers: { Z: "1", A: "2" },
  },
  builtinModelIds: ["glm-4", "glm-4-air"],
  personalModelIds: ["mine"],
  modelOrder: ["mine", "glm-4"],
  visibility: "visible",
};

const COMPLETE_PROVIDER_CONFIG = {
  group: "zai-family",
  access: { type: "api-key", apiKey: "sk-live" },
  api: { type: "openai-chat-completions", baseUrl: "https://api.example.com" },
  builtinModelIds: ["glm-4"],
};

// ---------------------------------------------------------------------------
// 合法 Provider 记录
// ---------------------------------------------------------------------------

addParse("provider-config/full", "provider-config", FULL_PROVIDER_CONFIG, () =>
  parseProviderConfig(FULL_PROVIDER_CONFIG),
);
addParse("provider-config/empty", "provider-config", {}, () => parseProviderConfig({}));
addParse(
  "provider-config/all-null",
  "provider-config",
  {
    group: null,
    logo: null,
    access: null,
    api: null,
    builtinModelIds: null,
    personalModelIds: null,
    modelOrder: null,
    visibility: null,
  },
  () =>
    parseProviderConfig({
      group: null,
      logo: null,
      access: null,
      api: null,
      builtinModelIds: null,
      personalModelIds: null,
      modelOrder: null,
      visibility: null,
    }),
);
addParse(
  "provider-config/zhipu-access-sparse",
  "provider-config",
  { access: { type: "zhipu-account" }, visibility: "hidden" },
  () => parseProviderConfig({ access: { type: "zhipu-account" }, visibility: "hidden" }),
);
addParse(
  "provider-config/model-ids-trimmed",
  "provider-config",
  { builtinModelIds: [" glm-4 ", "glm-4-air"] },
  () => parseProviderConfig({ builtinModelIds: [" glm-4 ", "glm-4-air"] }),
);
addParse("provider-config/whitespace-model-id", "provider-config", { builtinModelIds: ["   "] }, () =>
  parseProviderConfig({ builtinModelIds: ["   "] }),
);

addParse(
  "provider-config-map/order-and-empty",
  "provider-config-map",
  [
    { providerId: "z-provider", config: { group: "bigmodel-family" } },
    { providerId: "a-provider", providerName: "A", enabled: false, config: {} },
  ],
  () =>
    parseProviderConfigMap([
      { providerId: "z-provider", config: { group: "bigmodel-family" } },
      { providerId: "a-provider", providerName: "A", enabled: false, config: {} },
    ]),
);
addParse("provider-config-map/empty", "provider-config-map", [], () => parseProviderConfigMap([]));
addParse(
  "provider-config-map/null-fields",
  "provider-config-map",
  [{ providerId: "p", templateId: null, providerName: null, config: { api: null } }],
  () =>
    parseProviderConfigMap([
      { providerId: "p", templateId: null, providerName: null, config: { api: null } },
    ]),
);

addParse(
  "mode-builtin-provider/valid",
  "mode-builtin-provider",
  {
    templateRules: [
      {
        templateId: "t1",
        templateNameMap: { "en-US": "Template 1", "zh-CN": "模板 1" },
        config: {
          logo: { type: "builtin", key: "k" },
          access: { type: "api-key", apiKeyManagementUrl: "https://keys.example.com" },
          api: { type: "anthropic-messages", baseUrl: "https://api.example.com" },
          builtinModelIds: ["m1"],
        },
      },
    ],
    providerRules: [
      {
        providerId: "p1",
        templateId: "t1",
        providerName: "P1",
        enabled: true,
        config: { group: "zai-family", builtinModelIds: ["m1"], visibility: "visible" },
      },
    ],
  },
  () =>
    parseModeBuiltinProviderConfigRules({
      templateRules: [
        {
          templateId: "t1",
          templateNameMap: { "en-US": "Template 1", "zh-CN": "模板 1" },
          config: {
            logo: { type: "builtin", key: "k" },
            access: { type: "api-key", apiKeyManagementUrl: "https://keys.example.com" },
            api: { type: "anthropic-messages", baseUrl: "https://api.example.com" },
            builtinModelIds: ["m1"],
          },
        },
      ],
      providerRules: [
        {
          providerId: "p1",
          templateId: "t1",
          providerName: "P1",
          enabled: true,
          config: { group: "zai-family", builtinModelIds: ["m1"], visibility: "visible" },
        },
      ],
    }),
);
addParse(
  "mode-builtin-provider/empty",
  "mode-builtin-provider",
  { templateRules: [], providerRules: [] },
  () => parseModeBuiltinProviderConfigRules({ templateRules: [], providerRules: [] }),
);

addParse(
  "personal-provider/valid",
  "personal-provider",
  {
    providerRules: [
      {
        providerId: "p1",
        providerName: "My Provider",
        enabled: true,
        config: {
          group: "standard-personal",
          api: { baseUrl: "https://editing.example.com", type: "openai-responses" },
          access: { type: "api-key", apiKey: "sk-personal" },
          personalModelIds: ["m1"],
          modelOrder: ["m1"],
          visibility: "visible",
        },
      },
    ],
  },
  () =>
    parsePersonalProviderConfigMap({
      providerRules: [
        {
          providerId: "p1",
          providerName: "My Provider",
          enabled: true,
          config: {
            group: "standard-personal",
            api: { baseUrl: "https://editing.example.com", type: "openai-responses" },
            access: { type: "api-key", apiKey: "sk-personal" },
            personalModelIds: ["m1"],
            modelOrder: ["m1"],
            visibility: "visible",
          },
        },
      ],
    }),
);
addParse("personal-provider/empty", "personal-provider", { providerRules: [] }, () =>
  parsePersonalProviderConfigMap({ providerRules: [] }),
);

addParse(
  "provider-template-map/order",
  "provider-template-map",
  [
    { templateId: "z", templateNameMap: { "zh-CN": "Z" }, config: {} },
    { templateId: "a", templateNameMap: {}, config: { builtinModelIds: ["m"] } },
  ],
  () =>
    parseProviderTemplateMap([
      { templateId: "z", templateNameMap: { "zh-CN": "Z" }, config: {} },
      { templateId: "a", templateNameMap: {}, config: { builtinModelIds: ["m"] } },
    ]),
);
addParse("provider-template-map/empty", "provider-template-map", [], () =>
  parseProviderTemplateMap([]),
);

addParse(
  "account-provider/valid",
  "account-provider",
  {
    "account:x": { builtinModelIds: ["glm-4"], access: { type: "zhipu-account", entitled: false } },
    "account:y": {},
  },
  () =>
    parseAccountProviderConfigMap({
      "account:x": { builtinModelIds: ["glm-4"], access: { type: "zhipu-account", entitled: false } },
      "account:y": {},
    }),
);
addParse("account-provider/empty", "account-provider", {}, () =>
  parseAccountProviderConfigMap({}),
);

// ---------------------------------------------------------------------------
// 合法 Model 记录
// ---------------------------------------------------------------------------

addParse(
  "model-config/full",
  "model-config",
  {
    enabled: true,
    properties: COMPLETE_PROPERTIES,
    optionSpecs: {
      reasoningLevel: { values: ["low", "medium", "high"], map: CEL_REASONING },
      maxOutputTokens: { max: 4_096, map: CEL_MAX_TOKENS },
    },
  },
  () =>
    parseModelConfig({
      enabled: true,
      properties: COMPLETE_PROPERTIES,
      optionSpecs: {
        reasoningLevel: { values: ["low", "medium", "high"], map: CEL_REASONING },
        maxOutputTokens: { max: 4_096, map: CEL_MAX_TOKENS },
      },
    }),
);
addParse("model-config/empty", "model-config", {}, () => parseModelConfig({}));
addParse(
  "model-config/partial-null",
  "model-config",
  {
    enabled: null,
    properties: {
      contextWindow: 8_192,
      inputFormat: { supportsText: true },
      outputFormat: null,
    },
    optionSpecs: { reasoningLevel: { values: ["low"] }, maxOutputTokens: null },
  },
  () =>
    parseModelConfig({
      enabled: null,
      properties: {
        contextWindow: 8_192,
        inputFormat: { supportsText: true },
        outputFormat: null,
      },
      optionSpecs: { reasoningLevel: { values: ["low"] }, maxOutputTokens: null },
    }),
);

addParse(
  "manual-model/full",
  "manual-model-config",
  { enabled: true, properties: MANUAL_PROPERTIES, optionSpecs: MANUAL_OPTION_SPECS },
  () =>
    manualModelConfigSchema.parse({
      enabled: true,
      properties: MANUAL_PROPERTIES,
      optionSpecs: MANUAL_OPTION_SPECS,
    }),
);

addParse(
  "mode-builtin-model-rules/full",
  "mode-builtin-model-rules",
  {
    modelRules: [{ modelMatch: "gpt-.*", config: { enabled: true } }],
    modelApiRules: [
      { modelMatch: "gpt-.*", apiTypeMatch: "openai-.*", config: { enabled: false } },
    ],
    providerSiteRules: [
      {
        modelMatch: "claude-.*",
        baseUrlMatch: "https://api\\.anthropic\\.com",
        apiTypeMatch: "anthropic-.*",
        config: { properties: { contextWindow: 200_000 } },
      },
    ],
    templateModelRules: [{ templateId: "t1", modelId: "m1", config: { enabled: true } }],
    builtinProviderModelRules: [{ providerId: "p1", modelId: "m1", config: { enabled: true } }],
  },
  () =>
    parseModeBuiltinModelConfigRules({
      modelRules: [{ modelMatch: "gpt-.*", config: { enabled: true } }],
      modelApiRules: [
        { modelMatch: "gpt-.*", apiTypeMatch: "openai-.*", config: { enabled: false } },
      ],
      providerSiteRules: [
        {
          modelMatch: "claude-.*",
          baseUrlMatch: "https://api\\.anthropic\\.com",
          apiTypeMatch: "anthropic-.*",
          config: { properties: { contextWindow: 200_000 } },
        },
      ],
      templateModelRules: [{ templateId: "t1", modelId: "m1", config: { enabled: true } }],
      builtinProviderModelRules: [{ providerId: "p1", modelId: "m1", config: { enabled: true } }],
    }),
);
addParse(
  "mode-builtin-model-rules/empty",
  "mode-builtin-model-rules",
  {
    modelRules: [],
    modelApiRules: [],
    providerSiteRules: [],
    templateModelRules: [],
    builtinProviderModelRules: [],
  },
  () =>
    parseModeBuiltinModelConfigRules({
      modelRules: [],
      modelApiRules: [],
      providerSiteRules: [],
      templateModelRules: [],
      builtinProviderModelRules: [],
    }),
);

addParse(
  "personal-model-rules/full",
  "personal-model-rules",
  {
    providerModelRules: [{ providerId: "p1", modelId: "m1", config: { enabled: true } }],
    manualProviderModelRules: [
      {
        providerId: "p1",
        modelId: "m2",
        config: { enabled: true, properties: MANUAL_PROPERTIES, optionSpecs: MANUAL_OPTION_SPECS },
      },
    ],
  },
  () =>
    parsePersonalModelConfigRules({
      providerModelRules: [{ providerId: "p1", modelId: "m1", config: { enabled: true } }],
      manualProviderModelRules: [
        {
          providerId: "p1",
          modelId: "m2",
          config: { enabled: true, properties: MANUAL_PROPERTIES, optionSpecs: MANUAL_OPTION_SPECS },
        },
      ],
    }),
);
addParse(
  "personal-model-rules/empty",
  "personal-model-rules",
  { providerModelRules: [], manualProviderModelRules: [] },
  () => parsePersonalModelConfigRules({ providerModelRules: [], manualProviderModelRules: [] }),
);

// ---------------------------------------------------------------------------
// 非法记录：未知字段 / 缺字段 / 类型 / 枚举 / 数字边界
// ---------------------------------------------------------------------------

addParse("invalid/provider-unknown-key", "provider-config", { surprise: 1 }, () =>
  parseProviderConfig({ surprise: 1 }),
);
addParse("invalid/provider-bad-group", "provider-config", { group: "unknown" }, () =>
  parseProviderConfig({ group: "unknown" }),
);
addParse(
  "invalid/provider-bad-logo",
  "provider-config",
  { logo: { type: "builtin", key: "" } },
  () => parseProviderConfig({ logo: { type: "builtin", key: "" } }),
);
addParse(
  "invalid/provider-bad-access-type",
  "provider-config",
  { access: { type: "oauth" } },
  () => parseProviderConfig({ access: { type: "oauth" } }),
);
addParse(
  "invalid/provider-bad-url",
  "provider-config",
  { api: { type: "openai-responses", baseUrl: "not a url" } },
  () => parseProviderConfig({ api: { type: "openai-responses", baseUrl: "not a url" } }),
);
addParse(
  "invalid/provider-unknown-api-key",
  "provider-config",
  { api: { surprise: true } },
  () => parseProviderConfig({ api: { surprise: true } }),
);
addParse(
  "invalid/provider-model-id-type",
  "provider-config",
  { builtinModelIds: ["ok", 1] },
  () => parseProviderConfig({ builtinModelIds: ["ok", 1] }),
);
addParse(
  "invalid/provider-logo-type",
  "provider-config",
  { logo: { type: "custom", key: "k" } },
  () => parseProviderConfig({ logo: { type: "custom", key: "k" } }),
);
addParse(
  "invalid/provider-map-enabled-null",
  "provider-config-map",
  [{ providerId: "p", enabled: null, config: {} }],
  () => parseProviderConfigMap([{ providerId: "p", enabled: null, config: {} }]),
);
addParse(
  "invalid/template-name-null",
  "provider-template-map",
  [{ templateId: "t", templateNameMap: { "zh-CN": null }, config: {} }],
  () =>
    parseProviderTemplateMap([
      { templateId: "t", templateNameMap: { "zh-CN": null }, config: {} },
    ]),
);
addParse(
  "invalid/provider-site-api-type-null",
  "mode-builtin-model-rules",
  {
    modelRules: [],
    modelApiRules: [],
    providerSiteRules: [{ modelMatch: "m", baseUrlMatch: "b", apiTypeMatch: null, config: {} }],
    templateModelRules: [],
    builtinProviderModelRules: [],
  },
  () =>
    parseModeBuiltinModelConfigRules({
      modelRules: [],
      modelApiRules: [],
      providerSiteRules: [
        { modelMatch: "m", baseUrlMatch: "b", apiTypeMatch: null, config: {} },
      ],
      templateModelRules: [],
      builtinProviderModelRules: [],
    }),
);

addParse(
  "invalid/provider-map-missing-config",
  "provider-config-map",
  [{ providerId: "p" }],
  () => parseProviderConfigMap([{ providerId: "p" }]),
);
addParse(
  "invalid/provider-map-unknown-key",
  "provider-config-map",
  [{ providerId: "p", config: {}, extra: 1 }],
  () => parseProviderConfigMap([{ providerId: "p", config: {}, extra: 1 }]),
);
addParse(
  "invalid/provider-map-empty-provider-id",
  "provider-config-map",
  [{ providerId: "", config: {} }],
  () => parseProviderConfigMap([{ providerId: "", config: {} }]),
);
addParse(
  "invalid/provider-map-duplicate",
  "provider-config-map",
  [
    { providerId: "p", config: {} },
    { providerId: "p", config: {} },
  ],
  () =>
    parseProviderConfigMap([
      { providerId: "p", config: {} },
      { providerId: "p", config: {} },
    ]),
);

addParse(
  "invalid/builtin-template-zhipu-access",
  "mode-builtin-provider",
  {
    templateRules: [
      { templateId: "t", templateNameMap: {}, config: { access: { type: "zhipu-account" } } },
    ],
    providerRules: [],
  },
  () =>
    parseModeBuiltinProviderConfigRules({
      templateRules: [
        { templateId: "t", templateNameMap: {}, config: { access: { type: "zhipu-account" } } },
      ],
      providerRules: [],
    }),
);
addParse(
  "invalid/builtin-template-api-key-field",
  "mode-builtin-provider",
  {
    templateRules: [
      { templateId: "t", templateNameMap: {}, config: { access: { type: "api-key", apiKey: "x" } } },
    ],
    providerRules: [],
  },
  () =>
    parseModeBuiltinProviderConfigRules({
      templateRules: [
        {
          templateId: "t",
          templateNameMap: {},
          config: { access: { type: "api-key", apiKey: "x" } },
        },
      ],
      providerRules: [],
    }),
);
addParse(
  "invalid/builtin-provider-standard-group",
  "mode-builtin-provider",
  { templateRules: [], providerRules: [{ providerId: "p", config: { group: "standard-personal" } }] },
  () =>
    parseModeBuiltinProviderConfigRules({
      templateRules: [],
      providerRules: [{ providerId: "p", config: { group: "standard-personal" } }],
    }),
);
addParse(
  "invalid/builtin-provider-personal-models",
  "mode-builtin-provider",
  {
    templateRules: [],
    providerRules: [{ providerId: "p", config: { group: "zai-family", personalModelIds: ["m"] } }],
  },
  () =>
    parseModeBuiltinProviderConfigRules({
      templateRules: [],
      providerRules: [
        { providerId: "p", config: { group: "zai-family", personalModelIds: ["m"] } },
      ],
    }),
);
addParse(
  "invalid/builtin-duplicate-ids",
  "mode-builtin-provider",
  {
    templateRules: [
      { templateId: "t", templateNameMap: {}, config: {} },
      { templateId: "t", templateNameMap: {}, config: {} },
    ],
    providerRules: [
      { providerId: "p", config: { group: "zai-family" } },
      { providerId: "p", config: { group: "zai-family" } },
    ],
  },
  () =>
    parseModeBuiltinProviderConfigRules({
      templateRules: [
        { templateId: "t", templateNameMap: {}, config: {} },
        { templateId: "t", templateNameMap: {}, config: {} },
      ],
      providerRules: [
        { providerId: "p", config: { group: "zai-family" } },
        { providerId: "p", config: { group: "zai-family" } },
      ],
    }),
);
addParse(
  "invalid/builtin-missing-provider-rules",
  "mode-builtin-provider",
  { templateRules: [] },
  () => parseModeBuiltinProviderConfigRules({ templateRules: [] }),
);

addParse(
  "invalid/personal-account-access",
  "personal-provider",
  { providerRules: [{ providerId: "account:x", config: { access: { type: "api-key" } } }] },
  () =>
    parsePersonalProviderConfigMap({
      providerRules: [{ providerId: "account:x", config: { access: { type: "api-key" } } }],
    }),
);
addParse(
  "invalid/personal-account-access-null",
  "personal-provider",
  { providerRules: [{ providerId: "account:x", config: { access: null } }] },
  () =>
    parsePersonalProviderConfigMap({
      providerRules: [{ providerId: "account:x", config: { access: null } }],
    }),
);
addParse(
  "invalid/personal-builtin-model-ids",
  "personal-provider",
  { providerRules: [{ providerId: "p", config: { builtinModelIds: ["m"] } }] },
  () =>
    parsePersonalProviderConfigMap({
      providerRules: [{ providerId: "p", config: { builtinModelIds: ["m"] } }],
    }),
);
addParse(
  "invalid/personal-group",
  "personal-provider",
  { providerRules: [{ providerId: "p", config: { group: "zai-family" } }] },
  () =>
    parsePersonalProviderConfigMap({
      providerRules: [{ providerId: "p", config: { group: "zai-family" } }],
    }),
);
addParse(
  "invalid/personal-duplicate",
  "personal-provider",
  {
    providerRules: [
      { providerId: "p", config: {} },
      { providerId: "p", config: {} },
    ],
  },
  () =>
    parsePersonalProviderConfigMap({
      providerRules: [
        { providerId: "p", config: {} },
        { providerId: "p", config: {} },
      ],
    }),
);

addParse(
  "invalid/template-map-duplicate",
  "provider-template-map",
  [
    { templateId: "t", templateNameMap: {}, config: {} },
    { templateId: "t", templateNameMap: {}, config: {} },
  ],
  () =>
    parseProviderTemplateMap([
      { templateId: "t", templateNameMap: {}, config: {} },
      { templateId: "t", templateNameMap: {}, config: {} },
    ]),
);
addParse(
  "invalid/template-locale",
  "provider-template-map",
  [{ templateId: "t", templateNameMap: { "ja-JP": "x" }, config: {} }],
  () => parseProviderTemplateMap([{ templateId: "t", templateNameMap: { "ja-JP": "x" }, config: {} }]),
);
addParse(
  "invalid/template-config-key",
  "provider-template-map",
  [{ templateId: "t", templateNameMap: {}, config: { visibility: "visible" } }],
  () =>
    parseProviderTemplateMap([
      { templateId: "t", templateNameMap: {}, config: { visibility: "visible" } },
    ]),
);

addParse(
  "invalid/account-access-extra",
  "account-provider",
  { "account:x": { access: { type: "zhipu-account", accountType: "zai" } } },
  () =>
    parseAccountProviderConfigMap({
      "account:x": { access: { type: "zhipu-account", accountType: "zai" } },
    }),
);
addParse(
  "invalid/account-access-type",
  "account-provider",
  { "account:x": { access: { type: "api-key" } } },
  () => parseAccountProviderConfigMap({ "account:x": { access: { type: "api-key" } } }),
);
addParse(
  "invalid/account-config-key",
  "account-provider",
  { "account:x": { group: "zai-family" } },
  () => parseAccountProviderConfigMap({ "account:x": { group: "zai-family" } }),
);

addParse(
  "invalid/model-unknown-key",
  "model-config",
  { enabled: true, surprise: 1 },
  () => parseModelConfig({ enabled: true, surprise: 1 }),
);
addParse(
  "invalid/model-context-window-zero",
  "model-config",
  { properties: { contextWindow: 0 } },
  () => parseModelConfig({ properties: { contextWindow: 0 } }),
);
addParse(
  "invalid/model-context-window-fraction",
  "model-config",
  { properties: { contextWindow: 1.5 } },
  () => parseModelConfig({ properties: { contextWindow: 1.5 } }),
);
addParse(
  "invalid/model-context-window-unsafe",
  "model-config",
  { properties: { contextWindow: 9_007_199_254_740_992 } },
  () => parseModelConfig({ properties: { contextWindow: 9_007_199_254_740_992 } }),
);
addParse(
  "invalid/model-empty-values",
  "model-config",
  { optionSpecs: { reasoningLevel: { values: [], map: CEL_REASONING } } },
  () => parseModelConfig({ optionSpecs: { reasoningLevel: { values: [], map: CEL_REASONING } } }),
);
addParse(
  "invalid/model-duplicate-values",
  "model-config",
  { optionSpecs: { reasoningLevel: { values: ["low", "low"], map: CEL_REASONING } } },
  () =>
    parseModelConfig({
      optionSpecs: { reasoningLevel: { values: ["low", "low"], map: CEL_REASONING } },
    }),
);
addParse(
  "invalid/model-cel",
  "model-config",
  { optionSpecs: { reasoningLevel: { values: ["low"], map: "unknownIdentifier" } } },
  () =>
    parseModelConfig({
      optionSpecs: { reasoningLevel: { values: ["low"], map: "unknownIdentifier" } },
    }),
);
addParse(
  "invalid/model-cel-not-object",
  "model-config",
  { optionSpecs: { maxOutputTokens: { map: "1 + 2" } } },
  () => parseModelConfig({ optionSpecs: { maxOutputTokens: { map: "1 + 2" } } }),
);
addParse(
  "invalid/model-input-format-type",
  "model-config",
  { properties: { inputFormat: { supportsText: "yes" } } },
  () => parseModelConfig({ properties: { inputFormat: { supportsText: "yes" } } }),
);

addParse(
  "invalid/manual-missing-required",
  "manual-model-config",
  { enabled: true },
  () => manualModelConfigSchema.parse({ enabled: true }),
);
addParse(
  "invalid/manual-missing-input-format",
  "manual-model-config",
  { properties: { ...MANUAL_PROPERTIES, inputFormat: undefined }, optionSpecs: MANUAL_OPTION_SPECS },
  () =>
    manualModelConfigSchema.parse({
      properties: { ...MANUAL_PROPERTIES, inputFormat: undefined },
      optionSpecs: MANUAL_OPTION_SPECS,
    }),
);
addParse(
  "invalid/manual-extra-property",
  "manual-model-config",
  {
    properties: { ...MANUAL_PROPERTIES, supportsText: true },
    optionSpecs: MANUAL_OPTION_SPECS,
  },
  () =>
    manualModelConfigSchema.parse({
      properties: { ...MANUAL_PROPERTIES, supportsText: true },
      optionSpecs: MANUAL_OPTION_SPECS,
    }),
);
addParse(
  "invalid/manual-max-map",
  "manual-model-config",
  {
    properties: MANUAL_PROPERTIES,
    optionSpecs: {
      reasoningLevel: { values: ["low"], map: CEL_REASONING },
      maxOutputTokens: { max: 1, map: CEL_MAX_TOKENS },
    },
  },
  () =>
    manualModelConfigSchema.parse({
      properties: MANUAL_PROPERTIES,
      optionSpecs: {
        reasoningLevel: { values: ["low"], map: CEL_REASONING },
        maxOutputTokens: { max: 1, map: CEL_MAX_TOKENS },
      },
    }),
);
addParse(
  "invalid/manual-properties-null",
  "manual-model-config",
  { properties: null, optionSpecs: MANUAL_OPTION_SPECS },
  () => manualModelConfigSchema.parse({ properties: null, optionSpecs: MANUAL_OPTION_SPECS }),
);

addParse(
  "invalid/builtin-model-bad-regex",
  "mode-builtin-model-rules",
  {
    modelRules: [{ modelMatch: "(", config: {} }],
    modelApiRules: [],
    providerSiteRules: [],
    templateModelRules: [],
    builtinProviderModelRules: [],
  },
  () =>
    parseModeBuiltinModelConfigRules({
      modelRules: [{ modelMatch: "(", config: {} }],
      modelApiRules: [],
      providerSiteRules: [],
      templateModelRules: [],
      builtinProviderModelRules: [],
    }),
);
addParse(
  "invalid/builtin-model-missing-group",
  "mode-builtin-model-rules",
  {
    modelRules: [],
    modelApiRules: [],
    providerSiteRules: [],
    templateModelRules: [],
  },
  () =>
    parseModeBuiltinModelConfigRules({
      modelRules: [],
      modelApiRules: [],
      providerSiteRules: [],
      templateModelRules: [],
    }),
);

addParse(
  "invalid/personal-model-duplicate-identity",
  "personal-model-rules",
  {
    providerModelRules: [{ providerId: "p", modelId: "m", config: {} }],
    manualProviderModelRules: [
      {
        providerId: "p",
        modelId: "m",
        config: { properties: MANUAL_PROPERTIES, optionSpecs: MANUAL_OPTION_SPECS },
      },
    ],
  },
  () =>
    parsePersonalModelConfigRules({
      providerModelRules: [{ providerId: "p", modelId: "m", config: {} }],
      manualProviderModelRules: [
        {
          providerId: "p",
          modelId: "m",
          config: { properties: MANUAL_PROPERTIES, optionSpecs: MANUAL_OPTION_SPECS },
        },
      ],
    }),
);
addParse(
  "invalid/personal-model-invalid-manual-config",
  "personal-model-rules",
  { providerModelRules: [], manualProviderModelRules: [{ providerId: "p", modelId: "m", config: {} }] },
  () =>
    parsePersonalModelConfigRules({
      providerModelRules: [],
      manualProviderModelRules: [{ providerId: "p", modelId: "m", config: {} }],
    }),
);

// ---------------------------------------------------------------------------
// validateComplete 记录
// ---------------------------------------------------------------------------

addValidation(
  "validate/provider-config/complete",
  "validate-provider-config",
  COMPLETE_PROVIDER_CONFIG,
  () => parseProviderConfig(COMPLETE_PROVIDER_CONFIG),
  (value) => value.validateComplete(),
);
addValidation(
  "validate/provider-config/missing-required",
  "validate-provider-config",
  {},
  () => parseProviderConfig({}),
  (value) => value.validateComplete(),
);
addValidation(
  "validate/provider-config/api-key-missing",
  "validate-provider-config",
  { group: "zai-family", access: { type: "api-key" }, api: { type: "openai-responses", baseUrl: "https://x" } },
  () =>
    parseProviderConfig({
      group: "zai-family",
      access: { type: "api-key" },
      api: { type: "openai-responses", baseUrl: "https://x" },
    }),
  (value) => value.validateComplete(),
);
addValidation(
  "validate/provider-config/zhipu-missing-fields",
  "validate-provider-config",
  {
    group: "zai-family",
    access: { type: "zhipu-account" },
    api: { type: "openai-responses", baseUrl: "https://x" },
  },
  () =>
    parseProviderConfig({
      group: "zai-family",
      access: { type: "zhipu-account" },
      api: { type: "openai-responses", baseUrl: "https://x" },
    }),
  (value) => value.validateComplete(),
);
addValidation(
  "validate/provider-config-map/partial",
  "validate-provider-config-map",
  [
    { providerId: "p1", config: COMPLETE_PROVIDER_CONFIG },
    { providerId: "p2", config: { group: "zai-family" } },
  ],
  () =>
    parseProviderConfigMap([
      { providerId: "p1", config: COMPLETE_PROVIDER_CONFIG },
      { providerId: "p2", config: { group: "zai-family" } },
    ]),
  (value) => value.validateComplete(),
);
addValidation(
  "validate/model-config/complete",
  "validate-model-config",
  {
    enabled: true,
    properties: COMPLETE_PROPERTIES,
    optionSpecs: {
      reasoningLevel: { values: ["low", "high"], map: CEL_REASONING },
      maxOutputTokens: { max: 4_096, map: CEL_MAX_TOKENS },
    },
  },
  () =>
    parseModelConfig({
      enabled: true,
      properties: COMPLETE_PROPERTIES,
      optionSpecs: {
        reasoningLevel: { values: ["low", "high"], map: CEL_REASONING },
        maxOutputTokens: { max: 4_096, map: CEL_MAX_TOKENS },
      },
    }),
  (value) => value.validateComplete(),
);
addValidation(
  "validate/model-config/partial",
  "validate-model-config",
  { properties: { inputFormat: { supportsText: true } } },
  () => parseModelConfig({ properties: { inputFormat: { supportsText: true } } }),
  (value) => value.validateComplete(),
);
addValidation(
  "validate/provider-template-map/always-valid",
  "validate-provider-template-map",
  [{ templateId: "t", templateNameMap: {}, config: {} }],
  () => parseProviderTemplateMap([{ templateId: "t", templateNameMap: {}, config: {} }]),
  (value) => value.validateComplete(),
);

const fixture = {
  version: 1,
  generatedBy: "scripts/generate-provider-config-fixtures.ts",
  records,
  validations,
};

mkdirSync(path.dirname(OUTPUT), { recursive: true });
writeFileSync(OUTPUT, `${JSON.stringify(fixture, null, 2)}\n`, "utf8");
console.log(`wrote ${records.length} parse records and ${validations.length} validations to ${OUTPUT}`);
