/**
 * 生成 `crates/rpc-wire/tests/fixtures/values.json`（RPC 线格式黄金字节）。
 *
 * 真源是 TS 实现（`packages/rpc/src/serialization.ts`）；Rust 侧只负责与这些字节对齐，
 * 见 `docs/specs/rpc-wire-format.md`。改动 TS 序列化后必须重跑本脚本并提交新黄金文件，
 * 不允许只改 Rust 让用例变绿。
 *
 * ```bash
 * node --import tsx scripts/generate-rpc-wire-fixtures.ts
 * cargo test -p rpc-wire
 * ```
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { BufferWriter, VSBuffer, serialize } from "../packages/rpc/src/index.js";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(ROOT, "crates", "rpc-wire", "tests", "fixtures", "values.json");

interface Case {
  name: string;
  value: unknown;
}

/** 覆盖：每个 tag、VQL 边界、负数、非 int32 数字、数组嵌套与 JSON 回退。 */
const CASES: Case[] = [
  { name: "undefined", value: undefined },
  { name: "null", value: null },
  { name: "true", value: true },
  { name: "false", value: false },
  { name: "empty-string", value: "" },
  { name: "ascii-string", value: "hello" },
  { name: "utf8-string", value: "模式 Mode 😀" },
  { name: "string-128-bytes", value: "x".repeat(128) },
  { name: "uint8array", value: new Uint8Array([0, 1, 255, 128]) },
  { name: "vsbuffer", value: VSBuffer.wrap(new Uint8Array([0, 1, 255, 128])) },
  { name: "int-0", value: 0 },
  { name: "int-1", value: 1 },
  { name: "int-127", value: 127 },
  { name: "int-128", value: 128 },
  { name: "int-16384", value: 16384 },
  { name: "int-max", value: 2147483647 },
  { name: "int-min", value: -2147483648 },
  { name: "int-negative-one", value: -1 },
  { name: "int-negative-large", value: -123456 },
  { name: "float", value: 1.5 },
  { name: "beyond-int32", value: 2147483648 },
  { name: "nan", value: Number.NaN },
  { name: "empty-array", value: [] },
  { name: "array-mixed", value: [1, "a", undefined, [2, 3]] },
  { name: "empty-object", value: {} },
  { name: "object-flat", value: { a: 1, b: "x", c: true } },
  { name: "object-nested", value: { task: { id: "t1", steps: [1, 2], meta: null } } },
  {
    name: "object-with-nested-bytes",
    value: { payload: new Uint8Array([7, 8, 9]) },
  },
];

const entries = CASES.map(({ name, value }) => {
  const writer = new BufferWriter();
  serialize(writer, value);
  return { name, hex: Buffer.from(writer.buffer.buffer).toString("hex") };
});

mkdirSync(path.dirname(OUTPUT), { recursive: true });
writeFileSync(OUTPUT, `${JSON.stringify({ generatedBy: "scripts/generate-rpc-wire-fixtures.ts", entries }, null, 2)}\n`);
console.log(`[rpc-wire] 写入 ${entries.length} 条样例 → ${path.relative(ROOT, OUTPUT)}`);
