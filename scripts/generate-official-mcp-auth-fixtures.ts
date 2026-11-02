/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明鉴权矩阵，拆分会降低可核对性。 */
/**
 * 生成 `official-mcp-auth` 的 Rust 对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/shared/src/official-mcp-auth.ts`
 *
 * 信任用例覆盖 dev loopback 开关、Mode API origin 归一化与凭证 URL 拒绝；
 * registry 用例由 Rust 侧桩解析器重放。
 *
 * ```bash
 * pnpm official-mcp-auth:fixtures
 * cargo test -p official-mcp-auth
 * ```
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  findOfficialMcpReservedHeaders,
  isOfficialMcpOriginTrusted,
  isOfficialMcpReservedHeaderName,
  readModeMcpRequestContextMeta,
  summarizeOfficialMcpIdentityHeaders,
} from "../packages/shared/src/official-mcp-auth.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(
  ROOT_DIR,
  "crates",
  "official-mcp-auth",
  "fixtures",
  "official-mcp-auth.json",
);

const HEADER_KEYS = [
  "Authorization",
  " x-bigmodel-authorization ",
  "Bigmodel-Target-Type",
  "BIGMODEL-ORGANIZATION",
  "bigmodel-project",
  "X-Coding-Plan-Api-Key",
  "mcp-session-id",
  "mcp-protocol-version",
  "X-Other",
];

const HEADER_MAPS: Array<{ name: string; headers: Record<string, string> }> = [
  {
    name: "reserved-and-other",
    headers: {
      Authorization: "a",
      "x-bigmodel-authorization": "b",
      "Bigmodel-Target-Type": "PERSONAL",
      "Bigmodel-Organization": "org",
      "Bigmodel-Project": "proj",
      "X-Coding-Plan-Api-Key": "key",
      "mcp-session-id": "s",
      "mcp-protocol-version": "1",
      "X-Other": "ok",
    },
  },
  {
    name: "duplicates-case-variants",
    headers: {
      authorization: "a",
      Authorization: "b",
      " MCP-SESSION-ID ": "c",
    },
  },
  { name: "empty", headers: {} },
];

const SUMMARIES: Array<{ name: string; headers: Record<string, string> }> = [
  {
    name: "team-with-target",
    headers: {
      "Bigmodel-Organization": "org",
      "Bigmodel-Project": "proj",
      "Bigmodel-Target-Type": "TEAM",
      Authorization: "secret",
    },
  },
  {
    name: "org-only",
    headers: { "bigmodel-organization": "org", authorization: "secret" },
  },
  {
    name: "empty-target",
    headers: {
      "Bigmodel-Target-Type": "",
      "Bigmodel-Organization": "org",
      "Bigmodel-Project": "proj",
    },
  },
  { name: "no-identity", headers: { "x-other": "v" } },
];

const TRUST_CASES = [
  { origin: "https://api.example.com", modeApiOrigin: "https://api.example.com" },
  { origin: "https://api.example.com/v1/", modeApiOrigin: "https://api.example.com" },
  { origin: "HTTPS://API.EXAMPLE.COM", modeApiOrigin: "https://api.example.com" },
  { origin: "https://api.example.com:443", modeApiOrigin: "https://api.example.com" },
  { origin: "https://sub.api.example.com", modeApiOrigin: "https://api.example.com" },
  { origin: "https://api.example.com", modeApiOrigin: undefined },
  { origin: "https://api.example.com", modeApiOrigin: "http://api.example.com" },
  { origin: "https://api.example.com", modeApiOrigin: "not a url" },
  { origin: "", modeApiOrigin: "https://api.example.com" },
  { origin: "   ", modeApiOrigin: "https://api.example.com" },
  {
    origin: "https://user:pass@api.example.com",
    modeApiOrigin: "https://api.example.com",
  },
  {
    origin: "http://127.0.0.1:3999",
    modeApiOrigin: undefined,
    devTrustedOriginsRaw: "http://127.0.0.1:3999",
  },
  {
    origin: "http://localhost:3999/path",
    devTrustedOriginsRaw: " , http://127.0.0.1:3999 , http://localhost:3999 , ",
    modeApiOrigin: undefined,
  },
  {
    origin: "http://localhost:3999",
    devTrustedOriginsRaw: "http://127.0.0.1:3999",
    modeApiOrigin: "https://api.example.com",
  },
  {
    origin: "https://localhost:3999",
    devTrustedOriginsRaw: "http://localhost:3999",
    modeApiOrigin: "https://api.example.com",
  },
  {
    origin: "http://[::1]:4000",
    devTrustedOriginsRaw: "http://[::1]:4000",
    modeApiOrigin: undefined,
  },
  {
    origin: "http://example.com",
    devTrustedOriginsRaw: "http://example.com",
    modeApiOrigin: undefined,
  },
];

const REGISTRY_CASES = [
  { resolver: "ok", value: "https://api.example.com", origin: "https://api.example.com" },
  { resolver: "ok", value: "https://api.example.com", origin: "https://other.example.com" },
  { resolver: "none", origin: "https://api.example.com" },
  { resolver: "error", origin: "https://api.example.com" },
  {
    resolver: "ok",
    value: "https://api.example.com",
    origin: "http://127.0.0.1:3999",
    devTrustedOriginsRaw: "http://127.0.0.1:3999",
  },
];

function resultOf(fn: () => unknown): unknown {
  return JSON.parse(JSON.stringify(fn())) as unknown;
}

function main(): void {
  const headers = HEADER_KEYS.map((key) => ({
    key,
    reserved: isOfficialMcpReservedHeaderName(key),
  }));
  const find = HEADER_MAPS.map((testCase) => ({
    name: testCase.name,
    headers: testCase.headers,
    expected: findOfficialMcpReservedHeaders(testCase.headers),
  }));
  const summaries = SUMMARIES.map((testCase) => ({
    name: testCase.name,
    headers: testCase.headers,
    expected: resultOf(() => summarizeOfficialMcpIdentityHeaders(testCase.headers)),
  }));
  const meta = [
    { name: "new-key", meta: { "com.mode/request-context": { a: 1 } } },
    {
      name: "legacy-key",
      meta: { "com.zcode/request-context": { legacy: true } },
    },
    {
      name: "new-null-falls-back",
      meta: { "com.mode/request-context": null, "com.zcode/request-context": 2 },
    },
    { name: "both-null", meta: { "com.mode/request-context": null, "com.zcode/request-context": null } },
    { name: "missing", meta: {} },
  ].map((testCase) => ({
    name: testCase.name,
    meta: testCase.meta,
    expected: resultOf(() => readModeMcpRequestContextMeta(testCase.meta as never) ?? null),
  }));
  const trust = TRUST_CASES.map((input) => ({
    input,
    expected: resultOf(() => isOfficialMcpOriginTrusted(input as never)),
  }));
  const registry = REGISTRY_CASES.map((input) => ({
    input,
    expected: resultOf(() =>
      isOfficialMcpOriginTrusted({
        origin: input.origin,
        modeApiOrigin: input.resolver === "ok" ? input.value : undefined,
        devTrustedOriginsRaw: input.devTrustedOriginsRaw,
        pluginId: "plugin-x",
      } as never),
    ),
  }));
  const fixture = { headers, find, summaries, meta, trust, registry };
  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(OUTPUT, `${JSON.stringify(fixture, null, 2)}\n`);
  process.stdout.write(`wrote ${path.relative(ROOT_DIR, OUTPUT)}\n`);
}

main();
