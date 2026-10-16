// Tauri 协议 TS 生成器：Rust 侧 `crates/protocol` 是唯一真源，本脚本把它的
// JSON Schema（`emit-schema` bin，schemars 产物）转成
// `packages/shared/src/generated/tauri-protocol.ts`。
//
// 为什么不用 typeshare：约定要「生成物提交进仓库 + CI 断言无漂移」，而 typeshare 需要在
// 开发机与 CI 装一个额外的 CLI。走 schemars + 本脚本的路线，生成器就在仓库里，
// 且 Rust 侧已有 schemars 依赖（协议类型本来就要 derive JsonSchema）。
//
// 产物含两部分：类型声明（schemars schema 转出）与**跨端字面量**（`$constants`，
// 例如深链事件名）。字面量也走生成而不是手抄，因为「事件名写错」是静默失效：
// 订阅装上了却永远收不到，只有 CI 的漂移断言能兜住。
//
// 用法：
//   node scripts/generate-tauri-protocol.mjs           # 写入生成物
//   node scripts/generate-tauri-protocol.mjs --check   # 只校验，有漂移则退出 1
//
// 环境变量：
//   MODE_PROTOCOL_SCHEMA_CMD  覆盖取 schema 的命令（默认 cargo run -q -p protocol --bin emit-schema）
//   MODE_RUST_TARGET          追加 `--target <三元组>`，只用于显式指定交叉目标；
//                             不设时按本机架构走宿主构建，本地 gnu 工具链由本脚本自己装配
import { spawnSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

import { cargoHostToolchainArgs, defaultGnuTarget, withCargoEnv } from "./rust-gnu-env.ts";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const outputPath = join(repoRoot, "packages", "shared", "src", "generated", "tauri-protocol.ts");
const checkOnly = process.argv.includes("--check");

function fail(message) {
  console.error(`[generate-tauri-protocol] ${message}`);
  process.exit(1);
}

function relFromRepo(path) {
  return relative(repoRoot, path).split(sep).join("/");
}

/** 跑 Rust 侧取 schema；失败时报出真实命令与 stderr，不静默产出空文件。 */
function readSchemaBundle() {
  const override = process.env.MODE_PROTOCOL_SCHEMA_CMD?.trim();
  const [command, ...baseArgs] = override
    ? override.split(/\s+/u)
    : ["cargo", "run", "-q", "-p", "protocol", "--bin", "emit-schema"];
  const explicitTarget = process.env.MODE_RUST_TARGET?.trim();
  // 未指定 target 时 cargo 按**宿主** toolchain 构建；本机宿主默认是 msvc，而下面会用
  // `+toolchain` 把它换成 gnu 宿主，所以「有效目标」就是本机架构对应的 gnu 三元组。
  // 它同时决定 linker 变量名与是否装配 llvm-mingw，不能再拿 explicitTarget 当它用。
  const effectiveTarget =
    explicitTarget ?? (process.platform === "win32" ? defaultGnuTarget() : undefined);
  const args = explicitTarget ? [...baseArgs, "--target", explicitTarget] : baseArgs;
  // 本地 gnu 路线自己装配，**不再要求调用方 export PATH / CARGO_TARGET_<triple>_LINKER**
  // （装配口径见 scripts/rust-gnu-env.ts；CI 的 msvc / Linux runner 上它退化成 no-op）。
  const env = withCargoEnv(process.env, { target: effectiveTarget });
  // `+toolchain` 只对 cargo 生效：MODE_PROTOCOL_SCHEMA_CMD 可能换成任意命令。
  const toolchainArgs = command === "cargo" ? cargoHostToolchainArgs(env, effectiveTarget) : [];
  const commandArgs = [...toolchainArgs, ...args];
  const result = spawnSync(command, commandArgs, {
    cwd: repoRoot,
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
    env,
  });
  if (result.error) {
    fail(`无法执行 ${command}：${result.error.message}`);
  }
  if (result.status !== 0) {
    fail(
      `${command} ${commandArgs.join(" ")} 失败（exit ${result.status}）：\n${result.stderr ?? ""}`,
    );
  }
  try {
    return JSON.parse(result.stdout);
  } catch (error) {
    fail(`schema 不是合法 JSON：${error.message}`);
  }
}

// ---------------------------------------------------------------- schema → TS

/** `#/definitions/Foo` → `Foo`；其它 ref 形式一律报错而不是猜。 */
function refName(ref) {
  const match = /^#\/definitions\/([\w.]+)$/u.exec(ref);
  if (!match) fail(`无法解析 $ref：${ref}`);
  return match[1];
}

function isStringEnums(schema) {
  return schema.type === "string" && Array.isArray(schema.enum);
}

/** 单个 schema 子节点 → TS 类型表达式。 */
function tsType(schema, indent) {
  if (!schema) fail("遇到空的子 schema");
  if (schema.$ref) return refName(schema.$ref);
  if (Array.isArray(schema.oneOf)) {
    return schema.oneOf.map((variant) => tsType(variant, indent)).join(" | ");
  }
  if (isStringEnums(schema)) {
    return schema.enum.map((value) => JSON.stringify(value)).join(" | ");
  }
  // schemars 用 `type: ["string", "null"]` 表达 Option<T>，这里摊成联合。
  if (Array.isArray(schema.type)) {
    return schema.type.map((one) => tsType({ ...schema, type: one }, indent)).join(" | ");
  }
  switch (schema.type) {
    case "string":
      return "string";
    case "integer":
    case "number":
      return "number";
    case "boolean":
      return "boolean";
    case "null":
      return "null";
    case "array":
      return `${tsType(schema.items, indent)}[]`;
    case "object":
      return inlineObjectType(schema, indent);
    default:
      return fail(`不支持的 schema 形态：${JSON.stringify(schema).slice(0, 200)}`);
  }
}

/** 可选字段的 TS 类型。
 *
 * 约定：Rust 侧所有 `Option<T>` 字段都带 `#[serde(skip_serializing_if = "Option::is_none")]`
 * （见 crates/protocol/src/lib.rs），即「不出现」而非「出现为 null」。schemars 对此给的是
 * `type: ["T", "null"]`，但它无法区分「真的可空」与「跳过的 Option」，于是这里按约定把
 * 可选字段的 `null` 分支去掉——产物于是与 packages/shared/src/platform.ts 的手写声明逐字一致
 * （`error?: string` 而不是 `error?: string | null`）。
 * 若将来出现**必须**以 `null` 表达的可选字段，应先改这条约定，而不是让生成物悄悄变形。
 */
function optionalTsType(schema, indent) {
  if (Array.isArray(schema.type) && schema.type.includes("null")) {
    const nonNull = schema.type.filter((one) => one !== "null");
    if (nonNull.length === 1) return tsType({ ...schema, type: nonNull[0] }, indent);
    return tsType({ ...schema, type: nonNull }, indent);
  }
  return tsType(schema, indent);
}

/** 内联对象（出现在 oneOf 变体里）：`{ a: string; b?: number }`。 */
function inlineObjectType(schema, indent) {
  const pad = " ".repeat(indent);
  const inner = " ".repeat(indent + 2);
  const required = new Set(schema.required ?? []);
  const lines = Object.entries(schema.properties ?? {}).map(
    ([name, child]) =>
      `${inner}${JSON.stringify(name)}${required.has(name) ? "" : "?"}: ${
        required.has(name) ? tsType(child, indent + 2) : optionalTsType(child, indent + 2)
      };`,
  );
  return `{\n${lines.join("\n")}\n${pad}}`;
}

/** 常量键：schema bundle 里以 `$` 开头的那一项，值形如 `{ NAME: "literal" }`。
 *
 * 单列成一个键而不是混进类型里，是为了让「哪些是类型、哪些是字面量」在数据层就分开：
 * 渲染顺序与转义规则都不同，混在一起就要靠命名猜。 */
const CONSTANTS_KEY = "$constants";

/** 字面量声明：`export const NAME = "value";`。
 *
 * 只接受字符串字面量：TS 生成物是给人读的协议面，放数字/布尔会诱使两边开始
 * 往这里塞可变的配置值，那就不是「跨端契约」了。 */
function tsConstantsDeclaration(constants) {
  const lines = Object.entries(constants).map(([name, value]) => {
    if (typeof value !== "string") {
      return fail(`$constants.${name} 不是字符串字面量：${JSON.stringify(value)}`);
    }
    return `export const ${name} = ${JSON.stringify(value)};`;
  });
  return lines;
}

/** 顶层声明：对象 → interface，其余 → type 别名。 */
function tsDeclaration(name, schema) {
  if (schema.oneOf || isStringEnums(schema)) {
    return `export type ${name} = ${tsType(schema, 0)};`;
  }
  if (schema.type === "object") {
    const required = new Set(schema.required ?? []);
    const lines = Object.entries(schema.properties ?? {}).map(
      ([field, child]) =>
        `  ${JSON.stringify(field)}${required.has(field) ? "" : "?"}: ${
          required.has(field) ? tsType(child, 2) : optionalTsType(child, 2)
        };`,
    );
    return `export interface ${name} {\n${lines.join("\n")}\n}`;
  }
  return `export type ${name} = ${tsType(schema, 0)};`;
}

function renderModule(bundle) {
  const constants = bundle[CONSTANTS_KEY] ?? {};
  const typeNames = Object.keys(bundle)
    .filter((name) => name !== CONSTANTS_KEY)
    .sort();
  const constantDeclarations = tsConstantsDeclaration(constants);
  const declarations = typeNames.map((name) => tsDeclaration(name, bundle[name]));
  return [
    "// 本文件由 scripts/generate-tauri-protocol.mjs 生成，请勿手改。",
    "//",
    "// 真源：crates/protocol（Rust，schemars）。改动协议请改 Rust 侧类型后重新生成：",
    "//   node scripts/generate-tauri-protocol.mjs",
    "// CI 用 `--check` 断言生成物无漂移。",
    "",
    ...(constantDeclarations.length > 0 ? [...constantDeclarations, ""] : []),
    ...declarations.flatMap((declaration) => [declaration, ""]),
  ]
    .join("\n")
    .replace(/\n+$/u, "\n");
}

// ---------------------------------------------------------------------- main

const generated = renderModule(readSchemaBundle());
const current = (() => {
  try {
    return readFileSync(outputPath, "utf8");
  } catch {
    return null;
  }
})();

/** 比对用的规范化：只统一换行。
 *
 * 生成物是文本，CRLF 与 LF 的差异**不是**协议漂移。Windows 上 `core.autocrlf=true`
 * （Git for Windows 的默认值，GitHub 的 windows runner 也是）会把该文件检出成 CRLF，
 * 而生成器写 LF —— 2026-10-16 的发布正是被这个假阳性挡下的：本地 LF 通过、CI CRLF 失败，
 * 报的却是"漂移"，于是三版连发失败都指向了不存在的问题。 */
function normalizeEol(text) {
  return text.replace(/\r\n/gu, "\n");
}

/** 漂移时给出可诊断的信息：先区分「只有换行不同」，再指出第一处内容差异。
 * 只报一句"漂移"会让下一个遇到的人重走一遍定位路。 */
function describeDrift(currentText, generatedText) {
  if (normalizeEol(currentText) === generatedText) {
    return "差异仅在换行符（CRLF vs LF），内容一致——请检查检出配置，不要重新生成。";
  }
  const currentLines = normalizeEol(currentText).split("\n");
  const generatedLines = generatedText.split("\n");
  for (let i = 0; i < Math.max(currentLines.length, generatedLines.length); i += 1) {
    if (currentLines[i] !== generatedLines[i]) {
      return [
        `首个差异在第 ${i + 1} 行：`,
        `  当前：${JSON.stringify(currentLines[i] ?? "<EOF>")}`,
        `  应为：${JSON.stringify(generatedLines[i] ?? "<EOF>")}`,
      ].join("\n");
    }
  }
  return "未定位到具体差异（内容相同但字节不同）。";
}

/** 内容是否已与生成物一致（换行不计）。 */
function contentMatches(currentText) {
  return currentText !== null && normalizeEol(currentText) === generated;
}

if (checkOnly) {
  if (contentMatches(current)) {
    console.log(`[generate-tauri-protocol] 无漂移：${relFromRepo(outputPath)}`);
    process.exit(0);
  }
  fail(
    `${relFromRepo(outputPath)} 与 crates/protocol 漂移。\n` +
      (current === null ? "文件不存在。" : describeDrift(current, generated)) +
      "\n运行 `node scripts/generate-tauri-protocol.mjs` 重新生成并提交。",
  );
}

if (contentMatches(current)) {
  console.log(`[generate-tauri-protocol] 已是最新：${relFromRepo(outputPath)}`);
} else {
  writeFileSync(outputPath, generated);
  console.log(`[generate-tauri-protocol] 已写入 ${relFromRepo(outputPath)}`);
}
