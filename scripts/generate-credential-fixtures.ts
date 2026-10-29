/**
 * 生成凭据密文的 Rust 对拍数据。
 *
 * 真源是 TS 的 `createCredentialCipherProvider`（`packages/services/src/credential/providers/credentialCipherProvider.ts`）。
 * Rust 侧只能做到「解开 TS 产出的密文」，不允许改 Rust 去迁就自己生成的样例。
 *
 * ```bash
 * pnpm credential:fixtures
 * cargo test -p credential-store
 * ```
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { createCredentialCipherProvider } from "../packages/services/src/credential/providers/credentialCipherProvider.js";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT, "crates", "credential-store", "fixtures", "credential-cipher.json");

/** 对拍用固定 secret；Rust 测试用同一个值构造 `Cipher`。 */
const SECRET = "credential-fixture-secret";

const provider = createCredentialCipherProvider({
  env: { MODE_CREDENTIAL_SECRET: SECRET } as NodeJS.ProcessEnv,
});

const plaintexts = [
  "a",
  "hello world",
  "中文凭据",
  "emoji 🔐",
  "line\nbreak",
  'quote"back\\slash',
  "  surrounding spaces  ",
];

const encrypted = plaintexts.map((plaintext) => {
  const ciphertext = provider.encrypt(plaintext);
  // 自检：fixture 自身必须能被真源解开，否则样例没有意义。
  const roundTrip = provider.decrypt(ciphertext);
  if (roundTrip !== plaintext) {
    throw new Error(`provider round-trip mismatch for ${JSON.stringify(plaintext)}`);
  }
  return { plaintext, ciphertext };
});

// 空明文的密文段是空串，TS 解密会先撞上「密文格式非法」这一支；
// 单独记录，让 Rust 侧显式对齐这个行为。
const emptyCiphertext = provider.encrypt("");

const valid = encrypted[0]!.ciphertext;
const tamperedPayload = valid.slice("enc:v1:".length);
const [ivPart, tagPart, cipherPart] = tamperedPayload.split(".");
// 改**首字符**：base64 末字符的低位可能被解码器忽略，改末位不保证密文字节变化。
const flippedFirst = cipherPart![0] === "A" ? "B" : "A";
const tamperedCipher = `${ivPart}.${tagPart}.${flippedFirst}${cipherPart!.slice(1)}`;

const invalid = [
  { name: "missing-segments", value: "enc:v1:abc" },
  { name: "two-segments", value: "enc:v1:a.b" },
  { name: "empty-iv", value: "enc:v1:.b.c" },
  { name: "iv-length", value: "enc:v1:AAAA.AAAAAAAAAAAAAAAAAAAAAA.AAAA" },
  { name: "tag-length", value: "enc:v1:AAAAAAAAAAAAAAAA.AAAA.AAAA" },
  { name: "tampered-ciphertext", value: `enc:v1:${tamperedCipher}` },
].map((entry) => {
  let reason: string | null = null;
  try {
    provider.decrypt(entry.value);
  } catch (error) {
    reason = (error as Error).message;
  }
  if (reason === null) {
    throw new Error(`expected decrypt failure for ${entry.name}`);
  }
  return { ...entry, expectedError: reason };
});

mkdirSync(path.dirname(OUTPUT), { recursive: true });
writeFileSync(
  OUTPUT,
  `${JSON.stringify(
    {
      generatedBy: "scripts/generate-credential-fixtures.ts",
      secret: SECRET,
      encrypted,
      emptyPlaintextCiphertext: emptyCiphertext,
      plaintextPassThrough: ["legacy-plain", ""],
      invalid,
    },
    null,
    2,
  )}\n`,
);
console.log(
  `[credential] encrypted=${encrypted.length} invalid=${invalid.length} → ${path.relative(ROOT, OUTPUT)}`,
);
