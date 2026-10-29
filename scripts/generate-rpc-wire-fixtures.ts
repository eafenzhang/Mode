/**
 * 生成 `crates/rpc-wire/tests/fixtures/` 的黄金字节：
 *
 * - `values.json`：序列化层（7 个 tag、VQL、分派顺序）；
 * - `frames.json`：stdio 分帧（4 字节大端长度）、13 字节协议消息头、RPC 信封。
 *
 * 真源是 TS 实现（`packages/rpc/src/serialization.ts`、`protocol.ts`、`channels.shared.ts`）；
 * Rust 侧只负责与这些字节对齐，见 `docs/specs/rpc-wire-format.md`。改动 TS 实现后必须重跑
 * 本脚本并提交新黄金文件，不允许只改 Rust 让用例变绿。
 *
 * ```bash
 * pnpm rpc-wire:fixtures
 * cargo test -p rpc-wire
 * ```
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { BufferWriter, VSBuffer, serialize } from "../packages/rpc/src/index.js";
import {
  ProtocolMessage,
  ProtocolMessageType,
  writeProtocolMessage,
} from "../packages/rpc/src/protocol.js";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const FIXTURE_DIR = path.join(ROOT, "crates", "rpc-wire", "tests", "fixtures");

interface Case {
  name: string;
  value: unknown;
}

/** 覆盖：每个 tag、VQL 边界、负数、非 int32 数字、数组嵌套与 JSON 回退。 */
const VALUE_CASES: Case[] = [
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

function hex(bytes: Uint8Array): string {
  return Buffer.from(bytes).toString("hex");
}

function encodeValue(value: unknown): Uint8Array {
  const writer = new BufferWriter();
  serialize(writer, value);
  return writer.buffer.buffer;
}

function concat(...parts: Uint8Array[]): Uint8Array {
  return new Uint8Array(Buffer.concat(parts.map((part) => Buffer.from(part))));
}

/** stdio 分帧：`[4 字节大端长度][payload]`（与 `stdioFrames.ts` 同形）。 */
function stdioFrame(payload: Uint8Array): Uint8Array {
  const header = Buffer.alloc(4);
  header.writeUInt32BE(payload.byteLength, 0);
  return concat(header, payload);
}

/** 协议消息头：type(1) + id(4 BE) + ack(4 BE) + length(4 BE) + payload。 */
function protocolMessage(
  type: ProtocolMessageType,
  id: number,
  ack: number,
  data: Uint8Array,
): Uint8Array {
  return writeProtocolMessage(new ProtocolMessage(type, id, ack, VSBuffer.wrap(data))).buffer;
}

function rpcEnvelope(header: unknown[], body: unknown): Uint8Array {
  return concat(encodeValue(header), encodeValue(body));
}

const valueEntries = VALUE_CASES.map(({ name, value }) => ({ name, hex: hex(encodeValue(value)) }));

const messageEntries = [
  {
    name: "regular-with-data",
    hex: hex(protocolMessage(ProtocolMessageType.Regular, 7, 3, encodeValue("hi"))),
  },
  {
    name: "regular-empty-data",
    hex: hex(protocolMessage(ProtocolMessageType.Regular, 0, 0, new Uint8Array())),
  },
  {
    name: "keep-alive",
    hex: hex(protocolMessage(ProtocolMessageType.KeepAlive, 0, 5, new Uint8Array())),
  },
  {
    name: "unknown-type-42",
    hex: hex(protocolMessage(42 as ProtocolMessageType, 1, 2, new Uint8Array([1, 2]))),
  },
];

const stdioPayload = encodeValue({ text: "hello" });
const stdioEntries = [
  { name: "object-payload", payloadHex: hex(stdioPayload), hex: hex(stdioFrame(stdioPayload)) },
  {
    name: "empty-payload",
    payloadHex: "",
    hex: hex(stdioFrame(new Uint8Array())),
  },
];

const envelopeEntries = [
  {
    name: "promise-no-arg",
    hex: hex(rpcEnvelope([100, 1, "host", "describe"], undefined)),
  },
  {
    name: "promise-with-arg",
    hex: hex(rpcEnvelope([100, 2, "task", "sendPrompt"], { text: "hi" })),
  },
  {
    name: "event-listen",
    hex: hex(rpcEnvelope([102, 3, "mode-task", "onEvent"], { taskId: "t1" })),
  },
  { name: "promise-cancel", hex: hex(rpcEnvelope([101, 4], undefined)) },
  { name: "event-dispose", hex: hex(rpcEnvelope([103, 5], undefined)) },
];

/** 服务端回帧：Initialize 只有类型，其余带 id（见 channelServer 的 `send` 调用）。 */
const responseEntries = [
  { name: "initialize", hex: hex(rpcEnvelope([200], undefined)) },
  { name: "promise-success", hex: hex(rpcEnvelope([201, 1], { ok: true })) },
  {
    name: "promise-error",
    hex: hex(rpcEnvelope([202, 2], { message: "boom", name: "Error", stack: ["a", "b"] })),
  },
  { name: "promise-error-obj", hex: hex(rpcEnvelope([203, 3], { code: "E_FAIL" })) },
  { name: "event-fire", hex: hex(rpcEnvelope([204, 4], { progress: 0.5 })) },
];

mkdirSync(FIXTURE_DIR, { recursive: true });
writeFileSync(
  path.join(FIXTURE_DIR, "values.json"),
  `${JSON.stringify(
    { generatedBy: "scripts/generate-rpc-wire-fixtures.ts", entries: valueEntries },
    null,
    2,
  )}\n`,
);
writeFileSync(
  path.join(FIXTURE_DIR, "frames.json"),
  `${JSON.stringify(
    {
      generatedBy: "scripts/generate-rpc-wire-fixtures.ts",
      messages: messageEntries,
      stdio: stdioEntries,
      envelopes: envelopeEntries,
      responses: responseEntries,
    },
    null,
    2,
  )}\n`,
);
console.log(
  `[rpc-wire] values=${valueEntries.length} messages=${messageEntries.length} ` +
    `stdio=${stdioEntries.length} envelopes=${envelopeEntries.length} ` +
    `responses=${responseEntries.length} → ${path.relative(ROOT, FIXTURE_DIR)}`,
);
