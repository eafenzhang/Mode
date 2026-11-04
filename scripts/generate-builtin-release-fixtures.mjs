/**
 * 生成 `provider-config-source` 的 Built-in Release 编解码对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/provider-node/src/mode-builtin-release.ts`
 *
 * `valid` 使用仓库内真实 Release（`config/provider/mode-builtin.json`），
 * 覆盖全部字段的 encode 键序与 `serialize` 结果；`rejects` 用最小文档做
 * schema/退出 Provider 的接受-拒绝对拍。
 *
 * ```bash
 * node scripts/generate-builtin-release-fixtures.mjs
 * cargo test -p provider-config-source
 * ```
 */
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { tsImport } from "tsx/esm/api";

const root = resolve(import.meta.dirname, "..");
const output = resolve(root, "crates/provider-config-source/fixtures/builtin-release.json");

const { decodeModeBuiltinRelease, encodeModeBuiltinRelease, serializeModeBuiltinRelease } =
  await tsImport(
    pathToFileURL(resolve(root, "packages/provider-node/src/mode-builtin-release.ts")).href,
    import.meta.url,
  );

const EMPTY_MODEL_RULES = {
  modelRules: [],
  modelApiRules: [],
  providerSiteRules: [],
  templateModelRules: [],
  builtinProviderModelRules: [],
};

function minimalRelease() {
  return {
    schemaVersion: 1,
    revision: 1,
    config: {
      providerConfigRules: { templateRules: [], providerRules: [] },
      modelConfigRules: structuredClone(EMPTY_MODEL_RULES),
    },
  };
}

const raw = JSON.parse(
  await readFile(resolve(root, "config/provider/mode-builtin.json"), "utf8"),
);
const decoded = decodeModeBuiltinRelease(raw);

const rejects = [];
function expectReject(name, input) {
  try {
    decodeModeBuiltinRelease(input);
  } catch (error) {
    rejects.push({ name, input, message: error instanceof Error ? error.message : String(error) });
    return;
  }
  throw new Error(`${name}: 期望拒绝，实际成功`);
}

const zapi = minimalRelease();
zapi.config.providerConfigRules.providerRules.push({
  providerId: "builtin:zapi",
  config: { group: "zai-family" },
});
expectReject("retired-zapi-provider", zapi);

for (const [name, mutate] of [
  ["schema-version", (doc) => ({ ...doc, schemaVersion: 2 })],
  ["unknown-root-key", (doc) => ({ ...doc, extra: true })],
  ["unknown-config-key", (doc) => ({ ...doc, config: { ...doc.config, extra: true } })],
  ["negative-revision", (doc) => ({ ...doc, revision: -1 })],
  ["fractional-revision", (doc) => ({ ...doc, revision: 1.5 })],
  ["unsafe-revision", (doc) => ({ ...doc, revision: 2 ** 53 })],
  ["string-revision", (doc) => ({ ...doc, revision: "1" })],
  ["missing-config", (doc) => ({ schemaVersion: doc.schemaVersion, revision: doc.revision })],
  [
    "missing-model-rules",
    (doc) => ({
      ...doc,
      config: { providerConfigRules: doc.config.providerConfigRules },
    }),
  ],
]) {
  expectReject(name, mutate(minimalRelease()));
}

const prettySampleInput = minimalRelease();
prettySampleInput.revision = 9;
prettySampleInput.config.providerConfigRules.providerRules.push({
  providerId: "p1",
  providerName: "P1",
  config: { group: "zai-family", builtinModelIds: ["m1", "m2"] },
});
const prettySample = decodeModeBuiltinRelease(prettySampleInput);

const fixture = {
  valid: {
    input: raw,
    encoded: encodeModeBuiltinRelease(decoded),
    serialized: serializeModeBuiltinRelease(decoded),
    revision: decoded.revision,
  },
  // applyRemoteRelease 落盘格式：JSON.stringify(..., null, 2)，无尾随换行。
  prettySample: {
    input: prettySampleInput,
    pretty: JSON.stringify(encodeModeBuiltinRelease(prettySample), null, 2),
  },
  rejects,
};

await mkdir(resolve(output, ".."), { recursive: true });
await writeFile(output, `${JSON.stringify(fixture)}\n`, "utf8");
console.log(`wrote ${output}: rejects=${rejects.length}`);
