import assert from "node:assert/strict";
import { test } from "node:test";

import { parseStartupMarks } from "../measure-host-startup.js";

const line = (phase: string, elapsedMs: number): string =>
  `[mode-host-sidecar] startup_mark phase=${phase} elapsedMs=${elapsedMs}`;

test("解析一次启动的全部分段并保持顺序", () => {
  const text = [
    line("native_deps", 210),
    line("storage", 260),
    line("services", 700),
    line("ready", 825),
  ].join("\n");
  assert.deepEqual(parseStartupMarks(text), [
    { phase: "native_deps", elapsedMs: 210 },
    { phase: "storage", elapsedMs: 260 },
    { phase: "services", elapsedMs: 700 },
    { phase: "ready", elapsedMs: 825 },
  ]);
});

test("多次启动时只保留最后一组", () => {
  const text = [
    line("native_deps", 100),
    line("storage", 150),
    line("ready", 300),
    line("native_deps", 180),
    line("storage", 240),
    line("ready", 450),
  ].join("\n");
  assert.deepEqual(parseStartupMarks(text), [
    { phase: "native_deps", elapsedMs: 180 },
    { phase: "storage", elapsedMs: 240 },
    { phase: "ready", elapsedMs: 450 },
  ]);
});

test("忽略无关日志行与不带标记的旧构建", () => {
  const text = [
    "[mode-host-sidecar] native_deps sqlite=ok(3.53.1)",
    "[mode-host-sidecar] ready startupId=host-1",
    "some other line",
  ].join("\n");
  assert.deepEqual(parseStartupMarks(text), []);
});

test("兼容 CRLF 与行内有其它内容", () => {
  const text = `prefix ${line("native_deps", 5)} suffix\r\n${line("ready", 9)}\r\n`;
  assert.deepEqual(parseStartupMarks(text), [
    { phase: "native_deps", elapsedMs: 5 },
    { phase: "ready", elapsedMs: 9 },
  ]);
});
