// 脱敏（spec §7）：sk/Bearer/键名/域名/home/回环，先脱敏再落盘与比较。
import assert from "node:assert/strict";
import test from "node:test";

import { createRedactor } from "../src/redact.js";

test("sk- key、Bearer、键值对被脱敏", () => {
  const redactor = createRedactor();
  assert.equal(redactor.redactString("key is sk-abcdef1234567890"), "key is <redacted-key>");
  assert.equal(
    redactor.redactString("Authorization: Bearer abc.def.ghi"),
    "Authorization: Bearer <redacted-key>",
  );
  assert.equal(
    redactor.redactString("api_key: super-secret-value"),
    "api_key: <redacted>",
  );
  assert.equal(redactor.redactString("token=abc123"), "token=<redacted>");
});

test("home 与域名替换", () => {
  const redactor = createRedactor({
    homeDir: "C:\\Users\\alice",
    domains: ["internal.example"],
  });
  assert.equal(
    redactor.redactString("stored at C:\\Users\\alice\\.mode"),
    "stored at <home>\\.mode",
  );
  assert.equal(
    redactor.redactString("https://internal.example/api"),
    "https://<redacted-domain>/api",
  );
});

test("键名命中即整体脱敏", () => {
  const redactor = createRedactor();
  assert.deepEqual(redactor.redactValue({ apiKey: "topsecret", keep: 1 }), {
    apiKey: "<redacted>",
    keep: 1,
  });
  assert.deepEqual(redactor.redactValue({ nested: { refreshToken: "t" } }), {
    nested: { refreshToken: "<redacted>" },
  });
});

test("回环 URL 折叠", () => {
  const redactor = createRedactor();
  assert.equal(
    redactor.redactString("endpoint http://localhost:9000/stream"),
    "endpoint <loopback-url>/stream",
  );
});

test("非字符串原样保留", () => {
  const redactor = createRedactor();
  assert.deepEqual(redactor.redactValue({ count: 3, list: [1, "sk-abcdef123456"] }), {
    count: 3,
    list: [1, "<redacted-key>"],
  });
});
