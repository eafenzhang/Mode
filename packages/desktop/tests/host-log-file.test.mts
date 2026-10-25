/**
 * Host 文件日志（`<dataRoot>/v2/logs/host-<date>.log`）纯函数用例。
 * 背景（2026-10-25）：旧 Electron main EOL 后安装版无任何落盘日志，Host 死亡时零现场。
 */
import assert from "node:assert/strict";
import { mkdtemp, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  createHostLogFileSink,
  hostLogDir,
  hostLogFileName,
  isExpiredHostLogFile,
} from "../src/host-standalone/logFile.js";

const NL = String.fromCharCode(10);

test("日志目录固定在数据根 v2/logs", () => {
  assert.equal(hostLogDir(join("D:", "data")), join("D:", "data", "v2", "logs"));
});

test("文件名按本地日期 host-YYYY-MM-DD.log", () => {
  assert.equal(hostLogFileName(new Date(2026, 9, 5)), "host-2026-10-05.log");
});

test("超期判定：只认 host-YYYY-MM-DD.log 形态", () => {
  const now = new Date(2026, 9, 25);
  assert.equal(isExpiredHostLogFile("host-2026-10-01.log", now), true);
  assert.equal(isExpiredHostLogFile("host-2026-10-24.log", now), false);
  assert.equal(isExpiredHostLogFile("2026-10-01.log", now), false, "旧 Electron 线文件不动");
  assert.equal(isExpiredHostLogFile("host-notes.txt", now), false);
  assert.equal(isExpiredHostLogFile("host-ab-cd-ef.log", now), false);
});

test("超期判定：按自然日粒度（文件日期午夜起算）", () => {
  const now = new Date(2026, 9, 25, 12);
  assert.equal(isExpiredHostLogFile("host-2026-10-18.log", now), true);
  assert.equal(isExpiredHostLogFile("host-2026-10-19.log", now), false);
});

test("sink 写入落盘：追加、自动补换行、保持顺序", async () => {
  const root = await mkdtemp(join(tmpdir(), "mode-log-sink-"));
  const sink = await createHostLogFileSink(root);
  sink.write("line-1");
  sink.write(`line-2${NL}`);
  await new Promise((resolve) => setTimeout(resolve, 100));
  const raw = await readFile(join(hostLogDir(root), hostLogFileName()), "utf8");
  assert.ok(raw.includes(`line-1${NL}line-2`), `actual=${JSON.stringify(raw)}`);
});
