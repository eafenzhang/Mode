/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明供应商响应矩阵，拆分会降低可核对性。 */
/**
 * 生成 `provider-balance` 的 Rust 对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/usage-stats/providers/providerBalanceSpecs.ts`
 *
 * `resolve` 用例记录 baseUrl → {url, host}；`parse` 用例通过同一识别结果解析
 * 真实响应形状，未识别供应商记为 `expected: null`。
 *
 * ```bash
 * pnpm provider-balance:fixtures
 * cargo test -p provider-balance
 * ```
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { ProviderBalanceProvider } from "../packages/services/src/usage-stats/providers/providerBalanceProvider.js";
import { createProviderBalanceTargetResolver } from "../packages/services/src/usage-stats/providers/providerBalanceTargetResolver.js";
import { resolveProviderBalanceProvider } from "../packages/services/src/usage-stats/providers/providerBalanceSpecs.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(
  ROOT_DIR,
  "crates",
  "provider-balance",
  "fixtures",
  "provider-balance.json",
);

function hostOf(baseUrl: string): string {
  return new URL(baseUrl).hostname.toLowerCase();
}

const BASE_URLS = [
  "https://api.deepseek.com/v1",
  "https://sub.api.deepseek.com",
  "https://api.moonshot.cn/v1",
  "https://api.moonshot.ai/anthropic",
  "https://API.MOONSHOT.AI",
  "https://api.siliconflow.cn/v1",
  "https://api.siliconflow.com/v1",
  "https://api.stepfun.com/v1",
  "https://api.stepfun.ai/v1",
  "https://openrouter.ai/api/v1",
  "https://sub.openrouter.ai/x",
  "https://api.novita.ai/v3",
  "https://api.example.com/v1",
  "not a url",
] as const;

interface ParseCase {
  name: string;
  baseUrl: string;
  payload: unknown;
}

const PARSE_CASES: ParseCase[] = [
  {
    name: "deepseek-multi-currency",
    baseUrl: "https://api.deepseek.com/v1",
    payload: {
      is_available: false,
      balance_infos: [
        { currency: "CNY", total_balance: "12.50" },
        { currency: "USD", total_balance: 3 },
        { currency: "  ", total_balance: 1 },
        { total_balance: 2 },
        "ignored",
      ],
    },
  },
  {
    name: "deepseek-missing",
    baseUrl: "https://api.deepseek.com",
    payload: {},
  },
  {
    name: "deepseek-non-object",
    baseUrl: "https://api.deepseek.com",
    payload: [1, 2, 3],
  },
  {
    name: "moonshot-cny-data-wrapper",
    baseUrl: "https://api.moonshot.cn/v1",
    payload: { data: { available_balance: "100", voucher_balance: 20 } },
  },
  {
    name: "moonshot-usd-root-and-negative",
    baseUrl: "https://api.moonshot.ai",
    payload: { available_balance: -1, voucher_balance: null },
  },
  {
    name: "moonshot-null-remaining",
    baseUrl: "https://api.moonshot.ai/v1",
    payload: { data: { available_balance: null } },
  },
  {
    name: "siliconflow-cny-total-balance",
    baseUrl: "https://api.siliconflow.cn/v1",
    payload: { data: { totalBalance: "5.5", balance: 9, chargeBalance: 100 } },
  },
  {
    name: "siliconflow-usd-fallback-balance",
    baseUrl: "https://api.siliconflow.com/v1",
    payload: { data: { balance: 2 } },
  },
  {
    name: "stepfun",
    baseUrl: "https://api.stepfun.com/v1",
    payload: { balance: "7.25", total_cash_balance: 10 },
  },
  {
    name: "openrouter",
    baseUrl: "https://openrouter.ai/api/v1",
    payload: { data: { total_credits: 50, total_usage: 12.5 } },
  },
  {
    name: "openrouter-null-total",
    baseUrl: "https://openrouter.ai",
    payload: { data: { total_credits: null, total_usage: 10 } },
  },
  {
    name: "novita-scaled",
    baseUrl: "https://api.novita.ai/v3",
    payload: { availableBalance: "12500", cashBalance: 20000 },
  },
  {
    name: "novita-missing-total",
    baseUrl: "https://api.novita.ai/v3",
    payload: { availableBalance: 0 },
  },
  {
    name: "unknown-provider",
    baseUrl: "https://api.example.com/v1",
    payload: { available_balance: 1 },
  },
];

interface ServiceCase {
  name: string;
  providerId: string;
  now: number;
  resolver:
    | { kind: "absent" }
    | { kind: "null" }
    | { kind: "throw" }
    | {
        kind: "target";
        target: { providerName?: string | null; baseUrl?: string | null; apiKey?: string | null };
      };
  response:
    | { kind: "json"; payload: unknown; status?: number }
    | { kind: "status"; status: number }
    | { kind: "network" };
}

const SERVICE_CASES: ServiceCase[] = [
  {
    name: "empty-provider-id",
    providerId: "   ",
    now: 1_700_000_000_000,
    resolver: { kind: "target", target: { baseUrl: "https://api.deepseek.com", apiKey: "k" } },
    response: { kind: "json", payload: {} },
  },
  {
    name: "no-resolver",
    providerId: "p1",
    now: 1_700_000_000_001,
    resolver: { kind: "absent" },
    response: { kind: "json", payload: {} },
  },
  {
    name: "resolver-null",
    providerId: "p1",
    now: 1_700_000_000_002,
    resolver: { kind: "null" },
    response: { kind: "json", payload: {} },
  },
  {
    name: "resolver-throws",
    providerId: "p1",
    now: 1_700_000_000_003,
    resolver: { kind: "throw" },
    response: { kind: "json", payload: {} },
  },
  {
    name: "unknown-base-url",
    providerId: "p1",
    now: 1_700_000_000_004,
    resolver: {
      kind: "target",
      target: { providerName: " My Provider ", baseUrl: "https://api.example.com", apiKey: "k" },
    },
    response: { kind: "json", payload: {} },
  },
  {
    name: "empty-api-key",
    providerId: "p1",
    now: 1_700_000_000_005,
    resolver: { kind: "target", target: { baseUrl: "https://api.deepseek.com", apiKey: "  " } },
    response: { kind: "json", payload: {} },
  },
  {
    name: "unauthorized-401",
    providerId: "p1",
    now: 1_700_000_000_006,
    resolver: { kind: "target", target: { baseUrl: "https://api.deepseek.com", apiKey: "k" } },
    response: { kind: "status", status: 401 },
  },
  {
    name: "forbidden-403",
    providerId: "p1",
    now: 1_700_000_000_007,
    resolver: { kind: "target", target: { baseUrl: "https://api.deepseek.com", apiKey: "k" } },
    response: { kind: "status", status: 403 },
  },
  {
    name: "server-error",
    providerId: "p1",
    now: 1_700_000_000_008,
    resolver: { kind: "target", target: { baseUrl: "https://api.deepseek.com", apiKey: "k" } },
    response: { kind: "status", status: 500 },
  },
  {
    name: "network-error",
    providerId: "p1",
    now: 1_700_000_000_009,
    resolver: { kind: "target", target: { baseUrl: "https://api.deepseek.com", apiKey: "k" } },
    response: { kind: "network" },
  },
  {
    name: "deepseek-ok",
    providerId: "p1",
    now: 1_700_000_000_010,
    resolver: {
      kind: "target",
      target: {
        providerName: " DeepSeek ",
        baseUrl: "https://api.deepseek.com/v1",
        apiKey: " sk-test ",
      },
    },
    response: {
      kind: "json",
      payload: { is_available: true, balance_infos: [{ currency: "CNY", total_balance: 12 }] },
    },
  },
  {
    name: "empty-payload",
    providerId: "p1",
    now: 1_700_000_000_011,
    resolver: { kind: "target", target: { baseUrl: "https://api.deepseek.com", apiKey: "k" } },
    response: { kind: "json", payload: {} },
  },
  {
    name: "moonshot-ok",
    providerId: "p1",
    now: 1_700_000_000_012,
    resolver: {
      kind: "target",
      target: { providerName: "", baseUrl: "https://api.moonshot.ai", apiKey: "k" },
    },
    response: { kind: "json", payload: { data: { available_balance: 9 } } },
  },
  {
    name: "provider-name-fallback",
    providerId: "p1",
    now: 1_700_000_000_013,
    resolver: { kind: "target", target: { providerName: null, baseUrl: "https://api.deepseek.com", apiKey: "k" } },
    response: { kind: "json", payload: { balance_infos: [{ total_balance: 1 }] } },
  },
];

function apiClientFor(response: ServiceCase["response"]) {
  return {
    async request(): Promise<Response> {
      if (response.kind === "network") throw new Error("network down");
      if (response.kind === "status") {
        return new Response("error body", { status: response.status });
      }
      return new Response(JSON.stringify(response.payload), {
        status: response.status ?? 200,
        headers: { "content-type": "application/json" },
      });
    },
  } as never;
}

async function runServiceCase(testCase: ServiceCase): Promise<unknown> {
  const realNow = Date.now;
  Date.now = () => testCase.now;
  try {
    const options: Record<string, unknown> = { apiClient: apiClientFor(testCase.response) };
    if (testCase.resolver.kind !== "absent") {
      options.resolveTarget = async (providerId: string) => {
        if (testCase.resolver.kind === "null") return null;
        if (testCase.resolver.kind === "throw") throw new Error("resolver failed");
        return { providerId, ...testCase.resolver.target };
      };
    }
    const provider = new ProviderBalanceProvider(options as never);
    const snapshot = await provider.getSnapshot({ providerId: testCase.providerId });
    return JSON.parse(JSON.stringify(snapshot)) as unknown;
  } finally {
    Date.now = realNow;
  }
}

function balanceView(providers: unknown[]): Record<string, unknown> {
  return { providers };
}

function provider(over: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    providerId: "p1",
    providerName: " Name ",
    effectiveConfig: {
      access: { type: "api-key", apiKey: " k " },
      api: { baseUrl: "https://api.deepseek.com" },
    },
    ...over,
  };
}

async function buildTargetCases(): Promise<unknown[]> {
  const cases = [
    { name: "default", view: balanceView([provider()]), providerId: "p1" },
    { name: "missing-provider", view: balanceView([provider()]), providerId: "p2" },
    {
      name: "account-access",
      view: balanceView([
        provider({
          effectiveConfig: {
            access: { type: "zhipu-account" },
            api: { baseUrl: "https://x" },
          },
        }),
      ]),
      providerId: "p1",
    },
    {
      name: "access-missing",
      view: balanceView([provider({ effectiveConfig: {} })]),
      providerId: "p1",
    },
    { name: "empty-name", view: balanceView([provider({ providerName: "   " })]), providerId: "p1" },
    {
      name: "null-base-url",
      view: balanceView([
        provider({ effectiveConfig: { access: { type: "api-key", apiKey: "k" }, api: { baseUrl: null } } }),
      ]),
      providerId: "p1",
    },
    {
      name: "api-missing",
      view: balanceView([
        provider({ effectiveConfig: { access: { type: "api-key", apiKey: "k" } } }),
      ]),
      providerId: "p1",
    },
    {
      name: "null-api-key",
      view: balanceView([
        provider({ effectiveConfig: { access: { type: "api-key", apiKey: null }, api: {} } }),
      ]),
      providerId: "p1",
    },
    {
      name: "numeric-api-key",
      view: balanceView([
        provider({ effectiveConfig: { access: { type: "api-key", apiKey: 42 }, api: {} } }),
      ]),
      providerId: "p1",
    },
    {
      name: "multiple-providers",
      view: balanceView([
        provider({ providerId: "p0", providerName: "Other" }),
        provider({ providerId: "p1", providerName: "Second" }),
      ]),
      providerId: "p1",
    },
    { name: "case-sensitive-id", view: balanceView([provider()]), providerId: "P1" },
  ];
  const results: unknown[] = [];
  for (const testCase of cases) {
    const resolve = createProviderBalanceTargetResolver({
      getView: async () => testCase.view as never,
    });
    const target = await resolve(testCase.providerId);
    results.push({
      name: testCase.name,
      view: testCase.view,
      providerId: testCase.providerId,
      expected: target === null ? null : JSON.parse(JSON.stringify(target)),
    });
  }
  return results;
}

async function main(): Promise<void> {
  const resolve = BASE_URLS.map((baseUrl) => {
    const resolved = resolveProviderBalanceProvider(baseUrl);
    return {
      baseUrl,
      expected: resolved ? { url: resolved.url, host: hostOf(baseUrl) } : null,
    };
  });
  const parse = PARSE_CASES.map((testCase) => {
    const resolved = resolveProviderBalanceProvider(testCase.baseUrl);
    return {
      ...testCase,
      expected: resolved ? JSON.parse(JSON.stringify(resolved.parse(testCase.payload))) : null,
    };
  });
  const service: unknown[] = [];
  for (const testCase of SERVICE_CASES) {
    service.push({
      name: testCase.name,
      providerId: testCase.providerId,
      now: testCase.now,
      resolver: testCase.resolver,
      response: testCase.response,
      expected: await runServiceCase(testCase),
    });
  }
  const fixture = {
    resolve,
    parse,
    service,
    targets: await buildTargetCases(),
  };
  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(OUTPUT, `${JSON.stringify(fixture, null, 2)}\n`);
  process.stdout.write(`wrote ${path.relative(ROOT_DIR, OUTPUT)}\n`);
}

await main();
