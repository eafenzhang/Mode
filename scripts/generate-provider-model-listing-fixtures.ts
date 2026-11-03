/**
 * 生成 `provider-model-listing` 的 TS 对拍数据。
 *
 * 真源：`packages/provider/src/model-listing.ts`。
 * 请求构造与响应解析都是纯函数，fixture 可重复生成。
 *
 * ```bash
 * pnpm provider-model-listing:fixtures
 * cargo test -p provider-model-listing
 * ```
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  buildRemoteModelListRequest,
  parseRemoteModelListPage,
  type RemoteModelCatalogRequest,
  type ProviderApiType,
} from "../packages/provider/src/model-listing.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(
  ROOT_DIR,
  "crates",
  "provider-model-listing",
  "fixtures",
  "model-listing.json",
);

function buildCase(name: string, input: RemoteModelCatalogRequest) {
  try {
    return { name, input, result: buildRemoteModelListRequest(input) };
  } catch (error) {
    return {
      name,
      input,
      error: error instanceof Error ? error.message : String(error),
    };
  }
}

function parseCase(name: string, payload: unknown) {
  try {
    return { name, payload, result: parseRemoteModelListPage(payload) };
  } catch (error) {
    return {
      name,
      payload,
      error: error instanceof Error ? error.message : String(error),
    };
  }
}

const fixture = {
  build: [
    buildCase("openai-trailing-slash", {
      apiType: "openai-chat-completions",
      baseUrl: "https://api.example.com/v1/",
      apiKey: "sk-test",
      headers: null,
    }),
    buildCase("openai-root-no-path", {
      apiType: "openai-chat-completions",
      baseUrl: "https://api.example.com",
      apiKey: "sk",
      headers: null,
    }),
    buildCase("openai-no-key", {
      apiType: "openai-chat-completions",
      baseUrl: "https://api.example.com/base",
      apiKey: null,
      headers: null,
    }),
    buildCase("openai-explicit-header-priority", {
      apiType: "openai-responses",
      baseUrl: "https://api.example.com",
      apiKey: "sk-test",
      headers: { authorization: "draft", "X-Custom": "1" },
    }),
    buildCase("anthropic-adds-v1-and-limit", {
      apiType: "anthropic-messages",
      baseUrl: "https://gateway.example.com",
      apiKey: "sk-ant",
      headers: null,
    }),
    buildCase("anthropic-keeps-v1-case-insensitive", {
      apiType: "anthropic-messages",
      baseUrl: "https://gateway.example.com/V1/",
      apiKey: "sk-ant",
      headers: null,
    }),
    buildCase("anthropic-preserves-query-and-replaces-limit", {
      apiType: "anthropic-messages",
      baseUrl: "https://gateway.example.com/v1?team=a&limit=5",
      apiKey: "sk-ant",
      headers: null,
    }),
    buildCase("anthropic-explicit-headers-priority", {
      apiType: "anthropic-messages",
      baseUrl: "https://gateway.example.com",
      apiKey: "sk-ant",
      headers: { "x-api-key": "draft", "anthropic-version": "2030-01-01" },
    }),
    buildCase("anthropic-no-key", {
      apiType: "anthropic-messages",
      baseUrl: "https://gateway.example.com/v1",
      apiKey: null,
      headers: null,
    }),
    buildCase("error-empty-base-url", {
      apiType: "openai-chat-completions",
      baseUrl: "   ",
      apiKey: null,
      headers: null,
    }),
    buildCase("error-unsupported-protocol", {
      apiType: "openai-responses",
      baseUrl: "ftp://example.com/x",
      apiKey: null,
      headers: null,
    }),
    buildCase("error-invalid-url", {
      apiType: "openai-chat-completions",
      baseUrl: "not a url",
      apiKey: null,
      headers: null,
    }),
    buildCase("error-unsupported-api-type", {
      // 运行时才有非法值：绕过编译期联合类型，验证 default 分支的显式报错。
      apiType: "websocket" as ProviderApiType,
      baseUrl: "https://api.example.com",
      apiKey: null,
      headers: null,
    }),
  ],
  parse: [
    parseCase("openai-page-basic", {
      data: [{ id: "a" }, { id: " b " }, { id: 123 }, {}, null, { id: null }, { id: "c" }],
      has_more: false,
    }),
    parseCase("anthropic-page-with-cursor", {
      data: [{ id: "m1" }],
      has_more: true,
      last_id: "cursor-9",
    }),
    parseCase("has-more-without-last-id", {
      data: [{ id: "m1" }],
      has_more: true,
    }),
    parseCase("empty-last-id-no-cursor", {
      data: [{ id: "m1" }],
      has_more: true,
      last_id: "",
    }),
    parseCase("has-more-false-with-last-id", {
      data: [{ id: "m1" }],
      has_more: false,
      last_id: "ignored",
    }),
    parseCase("error-missing-data", { nope: true }),
    parseCase("error-data-not-array", { data: "x" }),
    parseCase("error-payload-array", [1, 2]),
    parseCase("error-payload-null", null),
    parseCase("error-payload-string", "x"),
  ],
};

mkdirSync(path.dirname(OUTPUT), { recursive: true });
writeFileSync(OUTPUT, `${JSON.stringify(fixture, null, 2)}\n`);
console.log(`wrote ${OUTPUT}`);
