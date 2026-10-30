/* oxlint-disable eslint(max-lines) -- 对拍数据集中声明场景与桩序列，拆分会降低可核对性。 */
/**
 * 生成 `account-provider-overlay` 的 Rust 对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS，全部通过真实实现执行）：
 * - `packages/provider/src/account-provider-resolution.ts`
 * - `packages/services/src/model-provider/accountProviderConnectionResolver.ts`
 * - `packages/provider/src/sources.ts`
 *
 * ```bash
 * pnpm account-provider-overlay:fixtures
 * cmd //c "D:\\tools\\cargo-msvc.cmd test -p account-provider-overlay"
 * ```
 *
 * `scenarios[].steps[]` 在同一 resolver 实例上顺序执行（验证 previousScopes /
 * resetPrevious）；每步保存输入、桩调用序列，以及 connections / providers /
 * states / revision / 调用日志的规范 JSON 原文。Rust 侧用相同输入和桩重放，
 * 逐字节比较；错误按 message 原文比较。
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  createAccountProviderConfigResolver,
  resolveAccountProviderConfigs,
  type AccountProviderConnectionResult,
} from "../packages/provider/src/account-provider-resolution.js";
import { parseProviderConfigMap } from "../packages/provider/src/config/schema.js";
import { createAccountProviderConfigSnapshot } from "../packages/provider/src/sources.js";
import type { CodingPlanAvailabilityResult } from "../packages/services/src/model-provider/codingPlanProviderAvailability.js";
import {
  createAccountProviderConnectionResolver,
  type AccountProviderConnectionResolverOptions,
  type AccountProviderConnectionSettings,
} from "../packages/services/src/model-provider/accountProviderConnectionResolver.js";
import type { ProviderFamilyDomain } from "../packages/shared/src/model-provider-family.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(
  ROOT_DIR,
  "crates",
  "account-provider-overlay",
  "fixtures",
  "account-provider-overlay.json",
);
const CONFIG_REVISION = "builtin:fixture-rev";
const THROW_API_KEY = "<throw>";

type Family = ProviderFamilyDomain;
type ApiKeyStep = string | null | typeof THROW_API_KEY;

interface AvailabilityDirective {
  result?: CodingPlanAvailabilityResult;
  apiKeysAsModels?: true;
}

interface Step {
  settings: AccountProviderConnectionSettings[];
  identities?: Partial<Record<Family, (string | null)[]>>;
  apiKeys?: Record<string, ApiKeyStep[]>;
  availability?: Partial<Record<Family, Record<string, AvailabilityDirective>[]>>;
  reasons: string[];
  configured: unknown[];
  previous: unknown[];
  previousStates?: Record<string, unknown>;
}

interface StepExpect {
  connections: string | null;
  providers: string | null;
  states: string | null;
  revision: string | null;
  calls: string;
  error: string | null;
}

interface Scenario {
  name: string;
  steps: Step[];
}

interface ScenarioFixture extends Scenario {
  expect: StepExpect[];
}

interface ConfigCase {
  name: string;
  configured: unknown[];
  previous: unknown[];
  connections: AccountProviderConnectionResult[];
}

interface ConfigExpect {
  providers: string | null;
  error: string | null;
}

interface ConfigFixture extends ConfigCase {
  expect: ConfigExpect;
}

interface SnapshotCase {
  name: string;
  basedOn: string;
  providers: unknown[];
  states?: Record<string, unknown>;
}

interface SnapshotExpect {
  revision: string;
  hasStates: boolean;
  json: string;
}

interface SnapshotFixture extends SnapshotCase {
  expect: SnapshotExpect;
}

// ---------------------------------------------------------------------------
// Provider 输入小工具
// ---------------------------------------------------------------------------

function accountProvider(
  providerId: string,
  accountType: Family,
  mode: string,
  config: Record<string, unknown> = {},
): unknown {
  return {
    providerId,
    config: {
      access: { type: "zhipu-account", accountType, mode },
      ...config,
    },
  };
}

function apiKeyProvider(providerId: string): unknown {
  return {
    providerId,
    config: { access: { type: "api-key", apiKey: "x" } },
  };
}

function providerOverlay(providerId: string, entitled: boolean, models?: string[]): unknown {
  return {
    providerId,
    config: {
      access: { type: "zhipu-account", entitled },
      ...(models ? { builtinModelIds: models } : {}),
    },
  };
}

// ---------------------------------------------------------------------------
// 确定性桩
// ---------------------------------------------------------------------------

function pick<T>(values: readonly T[], index: number, label: string): T {
  const value = values[Math.min(index, values.length - 1)];
  if (value === undefined) throw new Error(`fixture 缺少 ${label}`);
  return value;
}

interface CallLog {
  settings: number;
  identities: Array<[Family, string | null]>;
  apiKeys: Array<[string, Family, string, boolean]>;
  availability: Array<{
    family: Family;
    providers: Array<{
      providerId: string;
      family: Family;
      planKind: string;
      apiKey: string | null;
    }>;
  }>;
}

function createScriptedOptions(): {
  options: AccountProviderConnectionResolverOptions;
  reset: (step: Step) => void;
  callsJson: () => string;
} {
  let script: Step = { settings: [], reasons: [], configured: [], previous: [] };
  let settingsCount = 0;
  let identityCounts = new Map<Family, number>();
  let apiKeyCounts = new Map<string, number>();
  let availabilityCounts = new Map<Family, number>();
  let calls: CallLog = { settings: 0, identities: [], apiKeys: [], availability: [] };

  const options: AccountProviderConnectionResolverOptions = {
    async readSettings() {
      calls.settings += 1;
      const value = pick(script.settings, settingsCount, "settings");
      settingsCount += 1;
      return structuredClone(value);
    },
    async loadAccountIdentity(family) {
      const index = identityCounts.get(family) ?? 0;
      identityCounts.set(family, index + 1);
      const value = pick(script.identities?.[family] ?? [null], index, `identities.${family}`);
      calls.identities.push([family, value]);
      return value;
    },
    async loadCodingPlanApiKey(providerId, family, accountIdentity, forceRefresh) {
      calls.apiKeys.push([providerId, family, accountIdentity, forceRefresh]);
      const index = apiKeyCounts.get(providerId) ?? 0;
      apiKeyCounts.set(providerId, index + 1);
      const value = pick(script.apiKeys?.[providerId] ?? [null], index, `apiKeys.${providerId}`);
      if (value === THROW_API_KEY) throw new Error("stub api key failure");
      return value;
    },
    async resolveFamilyAvailability({ family, providers }) {
      calls.availability.push({
        family,
        providers: providers.map((provider) => ({
          providerId: provider.providerId,
          family: provider.family,
          planKind: provider.planKind,
          apiKey: provider.apiKey ?? null,
        })),
      });
      const index = availabilityCounts.get(family) ?? 0;
      availabilityCounts.set(family, index + 1);
      const directives = pick(
        script.availability?.[family] ?? [{}],
        index,
        `availability.${family}`,
      );
      const resolved: Record<string, CodingPlanAvailabilityResult> = {};
      for (const provider of providers) {
        const directive = directives[provider.providerId];
        if (!directive) {
          resolved[provider.providerId] = { kind: "unknown" };
          continue;
        }
        resolved[provider.providerId] = directive.apiKeysAsModels
          ? { kind: "available", models: [provider.apiKey ?? "null"] }
          : (directive.result ?? { kind: "unknown" });
      }
      return resolved;
    },
  };

  return {
    options,
    reset(step) {
      script = step;
      settingsCount = 0;
      identityCounts = new Map();
      apiKeyCounts = new Map();
      availabilityCounts = new Map();
      calls = { settings: 0, identities: [], apiKeys: [], availability: [] };
    },
    callsJson() {
      return JSON.stringify(calls);
    },
  };
}

// ---------------------------------------------------------------------------
// 场景执行
// ---------------------------------------------------------------------------

async function runScenario(scenario: Scenario): Promise<ScenarioFixture> {
  const scripted = createScriptedOptions();
  const connectionResolver = createAccountProviderConnectionResolver(scripted.options);
  let captured: readonly AccountProviderConnectionResult[] | undefined;
  const resolver = createAccountProviderConfigResolver(async (input) => {
    captured = await connectionResolver(input);
    return captured;
  });

  const expect: StepExpect[] = [];
  for (const step of scenario.steps) {
    scripted.reset(step);
    captured = undefined;
    const configuredProviders = parseProviderConfigMap(step.configured);
    const previousProviders = parseProviderConfigMap(step.previous);
    try {
      const { providers, states } = await resolver({
        configRevision: CONFIG_REVISION,
        configuredProviders,
        previousProviders,
        previousStates: step.previousStates,
        reasons: step.reasons,
      });
      const snapshot = createAccountProviderConfigSnapshot(CONFIG_REVISION, providers, states);
      expect.push({
        connections: captured ? JSON.stringify(captured) : null,
        providers: JSON.stringify(providers.toJSON()),
        states: JSON.stringify(states),
        revision: snapshot.revision,
        calls: scripted.callsJson(),
        error: null,
      });
    } catch (error) {
      expect.push({
        connections: captured ? JSON.stringify(captured) : null,
        providers: null,
        states: null,
        revision: null,
        calls: scripted.callsJson(),
        error: error instanceof Error ? error.message : String(error),
      });
    }
  }
  return { ...scenario, expect };
}

// ---------------------------------------------------------------------------
// 场景定义
// ---------------------------------------------------------------------------

const ZAI_INDIVIDUAL = { kind: "individual-coding-plan" } as const;
const ZAI_TEAM = {
  kind: "team-coding-plan",
  productId: "product-1",
  organizationId: "org-1",
  projectId: "project-1",
} as const;
const ZAI_TEAM_OTHER_ORG = {
  kind: "team-coding-plan",
  productId: "product-1",
  organizationId: "org-2",
  projectId: "project-1",
} as const;
const BM_TEAM = {
  kind: "team-coding-plan",
  productId: "product-bm",
  organizationId: "org-bm",
  projectId: "project-bm",
} as const;

const scenarios: Scenario[] = [
  {
    name: "status-matrix-and-family-grouping",
    steps: [
      {
        configured: [
          accountProvider("zai-start", "zai", "start-plan"),
          accountProvider("zai-individual", "zai", "individual-coding-plan"),
          accountProvider("zai-team", "zai", "team-coding-plan"),
          accountProvider("zai-offpeak", "zai", "off-peak"),
          accountProvider("bm-start", "bigmodel", "start-plan"),
          accountProvider("bm-individual", "bigmodel", "individual-coding-plan"),
        ],
        previous: [],
        settings: [
          {
            providerFamilyDomain: "zai",
            selections: { zai: ZAI_INDIVIDUAL, bigmodel: BM_TEAM },
          },
        ],
        identities: { zai: ["zai-user"], bigmodel: ["bm-user"] },
        apiKeys: {
          "zai-start": ["start-key"],
          "zai-individual": ["individual-key"],
          "bm-start": ["bm-start-key"],
          "bm-individual": ["bm-individual-key"],
        },
        availability: {
          zai: [
            {
              "zai-start": { result: { kind: "available" } },
              "zai-individual": { result: { kind: "available", models: ["glm-4", "glm-4-air"] } },
              "zai-team": { result: { kind: "pending", effectiveAt: 123, models: ["t1"] } },
            },
          ],
          bigmodel: [
            {
              "bm-start": { result: { kind: "unavailable", reason: "coding_plan_not_entitled" } },
            },
          ],
        },
        reasons: [],
      },
    ],
  },
  {
    name: "off-peak-no-match-and-team-key",
    steps: [
      {
        configured: [
          accountProvider("zai-individual", "zai", "individual-coding-plan"),
          accountProvider("zai-team", "zai", "team-coding-plan"),
          accountProvider("zai-offpeak", "zai", "off-peak"),
        ],
        previous: [],
        settings: [{ providerFamilyDomain: "zai", selections: { zai: ZAI_TEAM } }],
        identities: { zai: ["zai-user"] },
        apiKeys: { "zai-individual": ["individual-key"] },
        availability: {
          zai: [
            {
              "zai-individual": { result: { kind: "available", models: ["glm-4"] } },
              "zai-team": {
                result: { kind: "unavailable", reason: "coding_plan_not_connected" },
              },
            },
          ],
        },
        reasons: ["settings:oauth-login-entitlement"],
      },
      {
        configured: [
          accountProvider("zai-individual", "zai", "individual-coding-plan"),
          accountProvider("zai-team", "zai", "team-coding-plan"),
          accountProvider("zai-offpeak", "zai", "off-peak"),
        ],
        previous: [],
        settings: [{ providerFamilyDomain: "zai", selections: { zai: ZAI_TEAM } }],
        identities: { zai: ["zai-user"] },
        apiKeys: { "zai-individual": ["individual-key"] },
        availability: {
          zai: [
            {
              "zai-individual": { result: { kind: "available", models: ["glm-4"] } },
              "zai-team": {
                result: { kind: "unavailable", reason: "coding_plan_not_connected" },
              },
            },
          ],
        },
        reasons: ["explicit"],
      },
    ],
  },
  {
    name: "unresolved-families-and-no-identity",
    steps: [
      {
        configured: [
          accountProvider("zai-individual", "zai", "individual-coding-plan"),
          accountProvider("zai-start", "zai", "start-plan"),
          accountProvider("bm-individual", "bigmodel", "individual-coding-plan"),
        ],
        previous: [],
        settings: [
          {
            providerFamilyDomain: null,
            selections: {},
            unresolvedFamilies: ["zai"],
          },
        ],
        identities: { zai: [null], bigmodel: [null] },
        availability: {
          zai: [{ "zai-start": { result: { kind: "available" } } }],
          bigmodel: [{ "bm-individual": { result: { kind: "available" } } }],
        },
        reasons: [],
      },
      {
        configured: [
          accountProvider("bm-individual", "bigmodel", "individual-coding-plan"),
        ],
        previous: [],
        settings: [{ providerFamilyDomain: null, selections: {} }],
        identities: { bigmodel: [null] },
        availability: { bigmodel: [{ "bm-individual": { result: { kind: "available" } } }] },
        reasons: [],
      },
    ],
  },
  {
    name: "api-key-throw-folds-to-null",
    steps: [
      {
        configured: [
          accountProvider("zai-start", "zai", "start-plan"),
          accountProvider("zai-individual", "zai", "individual-coding-plan"),
        ],
        previous: [],
        settings: [
          { providerFamilyDomain: "zai", selections: { zai: ZAI_INDIVIDUAL } },
        ],
        identities: { zai: ["zai-user"] },
        apiKeys: { "zai-start": [THROW_API_KEY], "zai-individual": ["individual-key"] },
        availability: {
          zai: [
            {
              "zai-start": { apiKeysAsModels: true },
              "zai-individual": { apiKeysAsModels: true },
            },
          ],
        },
        reasons: [],
      },
    ],
  },
  {
    name: "reset-previous-and-last-known-good",
    steps: [
      {
        configured: [accountProvider("zai-individual", "zai", "individual-coding-plan")],
        previous: [],
        settings: [
          { providerFamilyDomain: "zai", selections: { zai: ZAI_INDIVIDUAL } },
        ],
        identities: { zai: ["user-a"] },
        apiKeys: { "zai-individual": ["key-a"] },
        availability: {
          zai: [
            {
              "zai-individual": {
                result: { kind: "unavailable", reason: "coding_plan_not_entitled" },
              },
            },
          ],
        },
        reasons: [],
      },
      {
        configured: [accountProvider("zai-individual", "zai", "individual-coding-plan")],
        previous: [providerOverlay("zai-individual", true, ["old-model"])],
        settings: [
          { providerFamilyDomain: "zai", selections: { zai: ZAI_INDIVIDUAL } },
        ],
        identities: { zai: ["user-b"] },
        apiKeys: { "zai-individual": ["key-b"] },
        availability: { zai: [{}] },
        previousStates: {
          "zai-individual": {
            availability: "available",
            entitled: true,
            unavailableReason: "not-entitled",
            current: true,
            connectionKey: "old-key",
            effectiveAt: 7,
          },
        },
        reasons: [],
      },
      {
        configured: [accountProvider("zai-individual", "zai", "individual-coding-plan")],
        previous: [providerOverlay("zai-individual", true, ["old-model"])],
        settings: [
          { providerFamilyDomain: "zai", selections: { zai: ZAI_INDIVIDUAL } },
        ],
        identities: { zai: ["user-b"] },
        apiKeys: { "zai-individual": ["key-b"] },
        availability: { zai: [{}] },
        previousStates: {
          "zai-individual": {
            availability: "available",
            entitled: true,
            unavailableReason: "not-entitled",
            current: true,
            connectionKey: "old-key",
            effectiveAt: 9,
          },
        },
        reasons: [],
      },
      {
        configured: [accountProvider("zai-individual", "zai", "individual-coding-plan")],
        previous: [providerOverlay("zai-individual", true, ["old-model"])],
        settings: [
          { providerFamilyDomain: "zai", selections: { zai: ZAI_INDIVIDUAL } },
        ],
        identities: { zai: ["user-c"] },
        apiKeys: { "zai-individual": ["key-c"] },
        availability: {
          zai: [{ "zai-individual": { result: { kind: "available" } } }],
        },
        previousStates: {
          "zai-individual": {
            availability: "available",
            entitled: true,
            current: true,
            connectionKey: "old-key",
            effectiveAt: 9,
          },
        },
        reasons: [],
      },
    ],
  },
  {
    name: "team-scope-reset",
    steps: [
      {
        configured: [accountProvider("zai-team", "zai", "team-coding-plan")],
        previous: [],
        settings: [{ providerFamilyDomain: "zai", selections: { zai: ZAI_TEAM } }],
        identities: { zai: ["team-user"] },
        apiKeys: {},
        availability: {
          zai: [{ "zai-team": { result: { kind: "available" } } }],
        },
        reasons: [],
      },
      {
        configured: [accountProvider("zai-team", "zai", "team-coding-plan")],
        previous: [providerOverlay("zai-team", true)],
        settings: [
          { providerFamilyDomain: "zai", selections: { zai: ZAI_TEAM_OTHER_ORG } },
        ],
        identities: { zai: ["team-user"] },
        apiKeys: {},
        availability: { zai: [{}] },
        previousStates: {
          "zai-team": {
            availability: "available",
            entitled: true,
            current: true,
            connectionKey: "old-team-key",
          },
        },
        reasons: [],
      },
    ],
  },
  {
    name: "stale-identity-fails-closed",
    steps: [
      {
        configured: [accountProvider("zai-individual", "zai", "individual-coding-plan")],
        previous: [],
        settings: [
          { providerFamilyDomain: "zai", selections: { zai: ZAI_INDIVIDUAL } },
        ],
        identities: { zai: ["user-a", "user-b"] },
        apiKeys: { "zai-individual": ["key-a"] },
        availability: {
          zai: [{ "zai-individual": { result: { kind: "available" } } }],
        },
        reasons: [],
      },
    ],
  },
  {
    name: "settings-change-fails-closed",
    steps: [
      {
        configured: [accountProvider("zai-individual", "zai", "individual-coding-plan")],
        previous: [],
        settings: [
          { providerFamilyDomain: "zai", selections: { zai: ZAI_INDIVIDUAL } },
          { providerFamilyDomain: "zai", selections: { zai: ZAI_TEAM } },
        ],
        identities: { zai: ["user-a"] },
        apiKeys: { "zai-individual": ["key-a"] },
        availability: {
          zai: [{ "zai-individual": { result: { kind: "available" } } }],
        },
        reasons: [],
      },
    ],
  },
];

const configCases: ConfigCase[] = [
  {
    name: "start-plan-normalizes-models",
    configured: [accountProvider("zai-start", "zai", "start-plan")],
    previous: [],
    connections: [
      { providerId: "zai-start", status: "available", models: [" a ", "", "b"] },
    ],
  },
  {
    name: "start-plan-available-without-models",
    configured: [accountProvider("zai-start", "zai", "start-plan")],
    previous: [],
    connections: [{ providerId: "zai-start", status: "available" }],
  },
  {
    name: "pending-start-plan-models",
    configured: [accountProvider("zai-start", "zai", "start-plan")],
    previous: [],
    connections: [
      {
        providerId: "zai-start",
        status: "pending",
        models: ["m1"],
        effectiveAt: 5,
      },
    ],
  },
  {
    name: "unknown-inherits-previous",
    configured: [accountProvider("zai-individual", "zai", "individual-coding-plan")],
    previous: [providerOverlay("zai-individual", true, ["old-model"])],
    connections: [{ providerId: "zai-individual", status: "unknown" }],
  },
  {
    name: "unknown-reset-previous",
    configured: [accountProvider("zai-individual", "zai", "individual-coding-plan")],
    previous: [providerOverlay("zai-individual", true, ["old-model"])],
    connections: [{ providerId: "zai-individual", status: "unknown", resetPrevious: true }],
  },
  {
    name: "unavailable-without-previous",
    configured: [accountProvider("zai-individual", "zai", "individual-coding-plan")],
    previous: [providerOverlay("zai-individual", true)],
    connections: [
      { providerId: "zai-individual", status: "unavailable", unavailableReason: "not-entitled" },
    ],
  },
  {
    name: "duplicate-connection",
    configured: [accountProvider("zai-individual", "zai", "individual-coding-plan")],
    previous: [],
    connections: [
      { providerId: "zai-individual", status: "available" },
      { providerId: "zai-individual", status: "available" },
    ],
  },
  {
    name: "unconfigured-provider",
    configured: [accountProvider("zai-individual", "zai", "individual-coding-plan")],
    previous: [],
    connections: [{ providerId: "ghost", status: "available" }],
  },
  {
    name: "non-account-provider",
    configured: [apiKeyProvider("api-provider")],
    previous: [],
    connections: [{ providerId: "api-provider", status: "available" }],
  },
];

const snapshotCases: SnapshotCase[] = [
  {
    name: "states-present",
    basedOn: CONFIG_REVISION,
    providers: [providerOverlay("zai-individual", true)],
    states: {
      "zai-individual": {
        availability: "available",
        entitled: true,
        current: true,
        connectionKey: "key-1",
      },
    },
  },
  {
    name: "states-absent",
    basedOn: CONFIG_REVISION,
    providers: [providerOverlay("zai-individual", false)],
  },
  {
    name: "empty-providers-and-empty-states",
    basedOn: "builtin:empty",
    providers: [],
    states: {},
  },
  {
    name: "empty-providers-no-states",
    basedOn: "builtin:empty",
    providers: [],
  },
];

// ---------------------------------------------------------------------------
// 输出
// ---------------------------------------------------------------------------

const scenarioFixtures = await Promise.all(scenarios.map(runScenario));

const configFixtures: ConfigFixture[] = configCases.map((entry) => {
  const configuredProviders = parseProviderConfigMap(entry.configured);
  const previousProviders = parseProviderConfigMap(entry.previous);
  try {
    const providers = resolveAccountProviderConfigs({
      configuredProviders,
      previousProviders,
      connections: entry.connections,
    });
    return {
      ...entry,
      expect: { providers: JSON.stringify(providers.toJSON()), error: null },
    };
  } catch (error) {
    return {
      ...entry,
      expect: {
        providers: null,
        error: error instanceof Error ? error.message : String(error),
      },
    };
  }
});

const snapshotFixtures: SnapshotFixture[] = snapshotCases.map((entry) => {
  const providers = parseProviderConfigMap(entry.providers);
  const snapshot = createAccountProviderConfigSnapshot(entry.basedOn, providers, entry.states);
  const json = JSON.stringify({
    revision: snapshot.revision,
    basedOnModeBuiltinRevision: snapshot.basedOnModeBuiltinRevision,
    providers: snapshot.providers.toJSON(),
    ...(snapshot.states ? { states: snapshot.states } : {}),
  });
  return {
    ...entry,
    expect: {
      revision: snapshot.revision,
      hasStates: "states" in snapshot,
      json,
    },
  };
});

const fixture = {
  version: 1,
  generatedBy: "scripts/generate-account-provider-overlay-fixtures.ts",
  configRevision: CONFIG_REVISION,
  scenarios: scenarioFixtures,
  configCases: configFixtures,
  snapshotCases: snapshotFixtures,
};

mkdirSync(path.dirname(OUTPUT), { recursive: true });
writeFileSync(OUTPUT, `${JSON.stringify(fixture, null, 2)}\n`, "utf8");
console.log(
  `wrote ${scenarioFixtures.length} scenarios (${scenarioFixtures.reduce(
    (total, scenario) => total + scenario.steps.length,
    0,
  )} steps), ${configFixtures.length} config cases, ${snapshotFixtures.length} snapshot cases to ${OUTPUT}`,
);
