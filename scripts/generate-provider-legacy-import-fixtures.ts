/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明旧 Personal Provider 导入矩阵，拆分会降低可核对性。 */
/**
 * 生成 `provider-legacy-migration` 的 Personal Provider 导入对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/model-provider/legacyPersonalProviderConfigImporter.ts`
 *
 * 每条用例是一组旧 Effective Store Provider；输出是
 * `providers.toJSON()` 与 `models.rules()`（`config` 已 `toJSON`）的 JSON 投影。
 *
 * ```bash
 * pnpm provider-legacy-import:fixtures
 * cargo test -p provider-legacy-migration
 * ```
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { importLegacyPersonalProviderConfig } from "../packages/services/src/model-provider/legacyPersonalProviderConfigImporter.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(
  ROOT_DIR,
  "crates",
  "provider-legacy-migration",
  "fixtures",
  "legacy-personal-import.json",
);

interface FixtureCase {
  name: string;
  providers: unknown[];
}

function legacy(over: Record<string, unknown>): Record<string, unknown> {
  return {
    id: "custom-a",
    name: "Custom A",
    endpoints: { anthropic: "", openai: "", gemini: "" },
    apiKey: "sk-test",
    models: [],
    createdAt: 1,
    updatedAt: 2,
    ...over,
  };
}

const cases: FixtureCase[] = [
  {
    name: "empty",
    providers: [],
  },
  {
    name: "builtin-templates-and-skipped-identities",
    providers: [
      legacy({ id: "builtin:bigmodel", apiKey: "  sk-bigmodel  ", models: ["ignored"] }),
      legacy({ id: "builtin:zai", apiKey: "   " }),
      legacy({ id: " builtin:zai ", apiKey: " sk-zai " }),
      legacy({ id: "builtin:other", apiKey: "sk-other" }),
      legacy({ id: "account:zai-individual-coding-plan", apiKey: "sk-account" }),
    ],
  },
  {
    name: "custom-provider-and-model-members",
    providers: [
      legacy({
        id: "  custom-a  ",
        name: " Custom A ",
        source: "custom",
        enabled: false,
        apiKey: "  sk-a  ",
        apiKeyUrl: "",
        headers: { "X-A": "1", "": "ignored" },
        apiFormat: "anthropic-messages",
        defaultKind: "anthropic",
        endpoints: { baseURL: "https://api.example.com/v1/", paths: {} },
        models: [
          "m1",
          { id: " m2 ", deleted: false, contextWindow: 64_000 },
          { id: "m3", deleted: true, contextWindow: 32_000 },
          { id: "m1", contextWindow: 1 },
          { id: "   ", contextWindow: 2 },
          " m4 ",
          { id: "m5", contextWindow: 10.5 },
          { id: "m6", contextWindow: 0 },
          { id: "m7", contextWindow: -5 },
        ],
      }),
    ],
  },
  {
    name: "source-filter-and-default-name",
    providers: [
      legacy({
        id: "custom-b",
        name: "custom-b",
        enabled: true,
        source: undefined,
        endpoints: { anthropic: "https://anthropic.example.com/v1" },
        models: ["b1"],
      }),
      legacy({ id: "custom-c", source: "builtin", models: ["c1"] }),
      legacy({ id: "custom-d", source: "models-dev", models: ["d1"] }),
      legacy({ id: "custom-e", source: "workspace", models: ["e1"] }),
    ],
  },
  {
    name: "empty-provider-id",
    providers: [
      legacy({ id: "   ", name: "Ignored", models: ["m1"] }),
      legacy({ id: "custom-f", name: "   ", models: [] }),
    ],
  },
  {
    name: "duplicate-provider-rules-replace-in-place",
    providers: [
      legacy({
        id: "custom-dup",
        name: "First",
        models: [{ id: "m1", contextWindow: 1 }],
      }),
      legacy({
        id: "custom-dup",
        name: "Second",
        enabled: false,
        models: [
          { id: "m1", contextWindow: 2 },
          { id: "m2", contextWindow: 3 },
        ],
      }),
    ],
  },
  {
    name: "api-base-url-fallbacks",
    providers: [
      legacy({
        id: "legacy-openai",
        endpoints: { openai: "https://openai.example.com/v1/" },
        apiFormat: "openai-chat-completions",
        models: [],
      }),
      legacy({
        id: "legacy-anthropic",
        endpoints: { anthropic: "https://anthropic.example.com" },
        apiFormat: "anthropic-messages",
        models: [],
      }),
      legacy({
        id: "legacy-empty",
        endpoints: { anthropic: "", openai: "", gemini: "" },
        apiFormat: "openai-responses",
        models: [],
      }),
    ],
  },
  {
    name: "empty-secrets-and-headers",
    providers: [
      legacy({
        id: "custom-empty-key",
        name: "Empty Key",
        apiKey: "   ",
        apiKeyUrl: "  https://example.com/key  ",
        headers: {},
        models: [{ id: "m1", contextWindow: 100 }],
      }),
    ],
  },
  {
    name: "raw-base-url-preserved-when-runtime-is-empty",
    providers: [
      legacy({
        id: "custom-space-url",
        endpoints: { baseURL: "   " },
        apiFormat: "anthropic-messages",
        models: [],
      }),
    ],
  },
];

function serializeProviders(update: ReturnType<typeof importLegacyPersonalProviderConfig>): unknown {
  return {
    providers: update.providers.toJSON(),
    models: update.models.rules().map((rule) => ({
      ...rule,
      config: rule.config.toJSON(),
    })),
  };
}

function main(): void {
  const fixtures = cases.map((testCase) => ({
    name: testCase.name,
    providers: testCase.providers,
    expected: serializeProviders(
      importLegacyPersonalProviderConfig({
        legacyProviders: testCase.providers as never,
      }),
    ),
  }));
  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(
    OUTPUT,
    `${JSON.stringify({ cases: fixtures }, null, 2)}\n`,
    "utf8",
  );
  process.stdout.write(`wrote ${OUTPUT}\n`);
}

main();
