/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明迁移用例，拆分会降低可核对性。 */
/**
 * 生成 `provider-legacy-migration` 的 Rust 对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/model-provider/legacyModelProviderSerialized.ts`
 *
 * 每条用例是一条旧 provider 记录；`legacyModelProviderListSchema` 解析失败记为
 * `valid:false`，成功则输出 `migrateLegacyModelProviderConfig` 的结果（JSON 化后
 * 省略 undefined 字段）。
 *
 * ```bash
 * pnpm provider-legacy:fixtures
 * cargo test -p provider-legacy-migration
 * ```
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  createModelProviderModelConfig,
  getDefaultModelProviderEndpointPathForKind,
  getDefaultModelSupportedFormatsFromApiFormat,
  getDefaultModelSupportedFormatsFromEndpoints,
  getModelProviderModelIds,
  legacyModelProviderListSchema,
  migrateLegacyModelProviderConfig,
  normalizeModelProviderConfiguredBaseUrl,
  resolveModelProviderApiFormat,
  resolveModelProviderDefaultKind,
  resolveModelProviderKindApiFormat,
  resolveModelProviderRuntimeBaseUrl,
  stripLegacyClaudeProviderMappings,
  stripModelProviderReasoningPatches,
} from "../packages/services/src/model-provider/legacyModelProviderSerialized.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(
  ROOT_DIR,
  "crates",
  "provider-legacy-migration",
  "fixtures",
  "legacy-providers.json",
);

interface FixtureCase {
  name: string;
  input: Record<string, unknown>;
  valid: boolean;
  migrated?: unknown;
}

function provider(over: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    id: "provider-a",
    name: "Provider A",
    endpoints: { anthropic: "https://api.example.com" },
    apiKey: "sk-test",
    models: ["model-1"],
    createdAt: 1_700_000_000_000,
    updatedAt: 1_700_000_000_001,
    ...over,
  };
}

function buildCases(): FixtureCase[] {
  return [
    {
      name: "minimal-anthropic",
      input: provider(),
      valid: true,
    },
    {
      name: "openai-chat-completions",
      input: provider({
        endpoints: { openai: "https://api.example.com/v1/chat/completions" },
        apiFormat: "openai-chat-completions",
        models: ["gpt-a"],
      }),
      valid: true,
    },
    {
      name: "both-endpoints-anthropic-default",
      input: provider({
        endpoints: {
          anthropic: "https://api.example.com/v1/messages",
          openai: "https://api.example.com/v1/chat/completions",
        },
      }),
      valid: true,
    },
    {
      name: "both-endpoints-openai-responses",
      input: provider({
        endpoints: {
          anthropic: "https://api.example.com/v1/messages",
          openai: "https://api.example.com/v1/responses",
        },
        apiFormat: "openai-responses",
      }),
      valid: true,
    },
    {
      name: "duplicate-absolute-url-collapsed",
      input: provider({
        endpoints: {
          anthropic:
            "https://api.example.com/v1/messages/https://api.example.com/v1/messages",
        },
      }),
      valid: true,
    },
    {
      name: "uppercase-scheme-not-collapsed",
      input: provider({
        endpoints: { anthropic: "HTTPS://api.example.com/v1/messages" },
      }),
      valid: true,
    },
    {
      name: "different-origins",
      input: provider({
        endpoints: {
          anthropic: "https://anthropic.example.com/v1/messages",
          openai: "https://openai.example.com/v1/chat/completions",
        },
      }),
      valid: true,
    },
    {
      name: "invalid-url-string",
      input: provider({ endpoints: { anthropic: "not a url" } }),
      valid: true,
    },
    {
      name: "openai-only-no-format",
      input: provider({
        endpoints: { openai: "https://api.example.com/v1" },
      }),
      valid: true,
    },
    {
      name: "gemini-only",
      input: provider({
        endpoints: { gemini: "https://gemini.example.com" },
      }),
      valid: true,
    },
    {
      name: "gemini-supported-format-disabled",
      input: provider({
        endpoints: { gemini: "https://gemini.example.com" },
        modelSupportedFormats: { "model-1": ["gemini"] },
      }),
      valid: true,
    },
    {
      name: "mixed-formats-drop-gemini",
      input: provider({
        modelSupportedFormats: { "model-1": ["anthropic", "gemini"] },
      }),
      valid: true,
    },
    {
      name: "supported-format-overrides",
      input: provider({
        models: ["model-1", "model-2"],
        modelSupportedFormats: {
          "model-1": ["openai", "responses"],
          "model-2": [],
        },
      }),
      valid: true,
    },
    {
      name: "display-names-and-trim",
      input: provider({
        models: ["  model-1  ", "model-2"],
        modelDisplayNames: { "  model-1  ": "  Model One  ", "model-2": "   " },
      }),
      valid: true,
    },
    {
      name: "optional-fields",
      input: provider({
        enabled: false,
        systemDisabledReason: "oauth_provider_inactive",
        source: "custom",
        modelsDevProviderId: "vendor-x",
        apiKeyRequired: false,
        headers: { "X-Test": "1" },
        logoUrl: "",
        apiKeyUrl: "https://example.com/key",
        providerMappings: {
          claude: { haiku: "h", sonnet: "s", opus: "o", reasoning: "r" },
          other: { nested: [1, 2] },
        },
      }),
      valid: true,
    },
    {
      name: "empty-provider-mappings",
      input: provider({ providerMappings: {} }),
      valid: true,
    },
    {
      name: "missing-models-defaults",
      input: (() => {
        const value = provider();
        delete value.models;
        return value;
      })(),
      valid: true,
    },
    {
      name: "empty-strings-preserved",
      input: provider({
        id: "",
        name: "",
        apiKey: "",
        endpoints: { anthropic: "", openai: "", gemini: "" },
        models: [],
      }),
      valid: true,
    },
    {
      name: "invalid-endpoints-missing",
      input: (() => {
        const value = provider();
        delete value.endpoints;
        return value;
      })(),
      valid: false,
    },
    {
      name: "invalid-api-format",
      input: provider({ apiFormat: "gemini-native" }),
      valid: false,
    },
    {
      name: "invalid-supported-format",
      input: provider({ modelSupportedFormats: { "model-1": ["grpc"] } }),
      valid: false,
    },
    {
      name: "invalid-claude-mapping",
      input: provider({
        providerMappings: { claude: { haiku: "h" } },
      }),
      valid: false,
    },
    {
      name: "invalid-models",
      input: provider({ models: "model-1" }),
      valid: false,
    },
    {
      name: "invalid-enabled",
      input: provider({ enabled: "yes" }),
      valid: false,
    },
  ];
}

function buildHelpers(): Record<string, unknown[]> {
  const providerShapes = [
    { apiFormat: "openai-responses", endpoints: {} },
    { defaultKind: "openai-compatible", endpoints: {} },
    { endpoints: { paths: { "openai-compatible": "/chat" } } },
    { endpoints: { paths: { openai: "/responses" } } },
    { endpoints: { paths: { anthropic: "/messages" } } },
    { endpoints: {} },
    {
      apiFormat: "openai-chat-completions",
      defaultKind: "anthropic",
      endpoints: { paths: { anthropic: "/messages", openai: "/responses" } },
    },
  ];
  const runtimeShapes = [
    { endpoints: { baseURL: "https://host", paths: { anthropic: "/v1/messages" } } },
    { endpoints: { baseURL: "https://host/", paths: { openai: "/responses" } } },
    { endpoints: { paths: { anthropic: "https://other/v1/messages" } } },
    {
      endpoints: { baseURL: "https://host/v1", paths: { "openai-compatible": "chat/completions" } },
    },
    { endpoints: { baseURL: "https://host/v1", paths: { openai: "/responses" } } },
    { endpoints: { baseURL: "https://host", paths: {} } },
    { endpoints: { baseURL: "", paths: { anthropic: "/v1/messages" } } },
    { endpoints: {} },
    { endpoints: { paths: {} } },
  ];
  return {
    apiFormat: providerShapes.map((input) => ({
      input,
      expected: resolveModelProviderApiFormat(input as never),
    })),
    defaultKind: providerShapes.map((input) => ({
      input,
      expected: resolveModelProviderDefaultKind(input as never),
    })),
    runtimeBaseUrl: runtimeShapes.flatMap((input) => [
      {
        input,
        expected: resolveModelProviderRuntimeBaseUrl(input as never),
      },
      {
        input,
        kind: "openai",
        expected: resolveModelProviderRuntimeBaseUrl(input as never, "openai" as never),
      },
    ]),
    configuredBaseUrl: [
      "https://host/path/",
      "  https://host/path//  ",
      "https://host/path/https://host/path",
      "not a url/",
      "",
    ].map((input) => ({ input, expected: normalizeModelProviderConfiguredBaseUrl(input) })),
    modelIds: [
      {
        models: [" a ", "b", { id: " c ", deleted: false }, { id: "d", deleted: true }, "", { id: "" }],
      },
      { models: [] },
    ].map(({ models }) => ({
      models,
      expected: getModelProviderModelIds({ models } as never),
    })),
    createModelConfig: [
      { id: " m " },
      { id: "full", name: "  Full  ", kinds: ["anthropic", "anthropic", "openai"], defaultKind: "openai", contextWindow: 1000.9 },
      { id: "zero", contextWindow: 0 },
      { id: "negative", contextWindow: -5, maxOutputTokens: 0 },
      {
        id: "all",
        name: "   ",
        kinds: [],
        defaultKind: "openai",
        modalities: { input: ["text", "image", "text"], output: ["pdf"] },
        reasoning: { defaultLevel: "high", levels: { high: {} } },
        priority: 1.5,
        disabledReason: "",
        supportsTools: false,
        supportsStructuredOutput: true,
        modified: false,
        deleted: false,
      },
      { id: "max", maxOutputTokens: 128, priority: 0 },
    ].map((params) => ({
      params,
      expected: createModelProviderModelConfig(params as never),
    })),
    stripReasoning: [
      {
        defaultLevel: "high",
        levels: {
          low: { anthropic: { set: [{ path: ["thinking"], value: true }] } },
          high: {},
        },
      },
      { defaultLevel: "", levels: {} },
      { levels: { only: { openai: { unset: [{ path: ["x"] }] } } } },
    ].map((reasoning) => ({
      reasoning,
      expected: stripModelProviderReasoningPatches(reasoning as never),
    })),
    stripClaude: [
      undefined,
      {},
      { claude: { haiku: "h", sonnet: "s", opus: "o", reasoning: "r" }, other: 1 },
    ].map((mappings) => ({
      mappings: mappings ?? null,
      expected: stripLegacyClaudeProviderMappings(mappings as never) ?? null,
    })),
    endpointPaths: ["anthropic", "openai", "openai-compatible"].map((kind) => ({
      kind,
      expected: getDefaultModelProviderEndpointPathForKind(kind as never),
    })),
    kindApiFormat: ["anthropic", "openai", "openai-compatible"].map((kind) => ({
      kind,
      expected: resolveModelProviderKindApiFormat(kind as never),
    })),
    defaultFormatsApiFormat: ["anthropic-messages", "openai-responses", "openai-chat-completions"].map(
      (apiFormat) => ({
        apiFormat,
        expected: getDefaultModelSupportedFormatsFromApiFormat(apiFormat as never),
      }),
    ),
    defaultFormatsEndpoints: [
      {
        baseURL: "https://x",
        paths: { anthropic: "/a", "openai-compatible": "/b", openai: "/c" },
      },
      { paths: { anthropic: "/a" } },
      {},
      { baseURL: "  " },
      { paths: {} },
      { baseURL: "https://x" },
    ].map((input) => ({
      input,
      expected: getDefaultModelSupportedFormatsFromEndpoints(input as never),
    })),
  };
}

function main(): void {
  const cases = buildCases();
  const fixture = {
    cases: cases.map((testCase) => {
      const parsed = legacyModelProviderListSchema.safeParse([testCase.input]);
      if (!parsed.success) {
        return { name: testCase.name, input: testCase.input, valid: false };
      }
      const migrated = migrateLegacyModelProviderConfig(parsed.data[0]!);
      return {
        name: testCase.name,
        input: testCase.input,
        valid: true,
        migrated: JSON.parse(JSON.stringify(migrated)) as unknown,
      };
    }),
    helpers: buildHelpers(),
  };
  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(OUTPUT, `${JSON.stringify(fixture, null, 2)}\n`);
  process.stdout.write(`wrote ${path.relative(ROOT_DIR, OUTPUT)} (${cases.length} cases)\n`);
}

main();
