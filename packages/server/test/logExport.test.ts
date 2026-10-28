/**
 * Web/fpk 日志导出（docs/specs/fnos-feature-parity-and-update.md P1）。
 *
 * 选择规则与桌面 exportLogs 同口径：host-*.log 最新 7 份；旧线/非 Host 进 omitted；
 * fpk 的 info.log 作为额外文件始终带上。
 */
import assert from "node:assert/strict";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  createLogBundle,
  defaultLogExportFileName,
  renderLogBundle,
  selectHostLogFiles,
  type LogBundleFile,
} from "../src/logExport.js";

function file(name: string, content: string): LogBundleFile {
  return { name, bytes: Buffer.from(content, "utf8") };
}

test("只选 host-*.log 的最新 7 份，其余进 omitted", () => {
  const files = [
    ...Array.from({ length: 9 }, (_, index) =>
      file(`host-2026-10-${String(index + 1).padStart(2, "0")}.log`, `day-${String(index + 1)}`),
    ),
    file("2026-10-15.log", "old-electron"),
    file("notes.txt", "ignored"),
  ];
  const selection = selectHostLogFiles(files);
  assert.equal(selection.included.length, 7);
  assert.equal(selection.included[0]?.name, "host-2026-10-03.log");
  assert.equal(selection.included[6]?.name, "host-2026-10-09.log");
  assert.deepEqual(
    selection.omitted.map((entry) => entry.name),
    ["2026-10-15.log", "host-2026-10-01.log", "host-2026-10-02.log"],
  );
});

test("额外文件（fpk info.log）追加在 Host 日志之后", () => {
  const selection = selectHostLogFiles([file("host-2026-10-09.log", "host")]);
  selection.included.push(file("info.log", "service stdout"));
  assert.deepEqual(
    selection.included.map((entry) => entry.name),
    ["host-2026-10-09.log", "info.log"],
  );
});

test("包格式包含 exportedAt/logDir/files/omitted 与分段标记", () => {
  const bundle = renderLogBundle("2026-10-27T00:00:00.000Z", "/tmp/logs", {
    included: [file("host-2026-10-09.log", "hello\n")],
    omitted: [{ name: "2026-10-08.log", size: 123 }],
    truncated: [],
  });
  assert.match(bundle, /^Mode logs export\n/u);
  assert.match(bundle, /exportedAt: 2026-10-27T00:00:00\.000Z/u);
  assert.match(bundle, /logDir: \/tmp\/logs/u);
  assert.match(bundle, /files: 1/u);
  assert.match(bundle, /omitted: 1/u);
  assert.match(bundle, /truncated: 0/u);
  assert.match(bundle, /omittedFile: 2026-10-08\.log \(123 bytes\)/u);
  assert.match(bundle, /===== host-2026-10-09\.log \(6 bytes\) =====\nhello\n/u);
});

test("超大文件只保留尾部，并在 manifest 标注 truncated", async () => {
  const root = await mkdtemp(join(tmpdir(), "mode-log-cap-"));
  try {
    const logDir = join(root, "logs");
    await mkdir(logDir, { recursive: true });
    await writeFile(join(logDir, "host-2026-10-09.log"), "abcdefghij", "utf8");
    const result = await createLogBundle({
      logDir,
      maxFileBytes: 4,
      maxTotalBytes: 8,
      now: new Date("2026-10-27T00:00:00.000Z"),
    });
    assert.ok(result);
    assert.match(result.bundle, /truncated: 1/u);
    assert.match(
      result.bundle,
      /truncatedFile: host-2026-10-09\.log \(included 4 of 10 bytes, tail\)/u,
    );
    assert.match(result.bundle, /===== host-2026-10-09\.log \(4 bytes\) =====\nghij\n/u);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("createLogBundle 读取目录与额外文件；无日志返回 null", async () => {
  const root = await mkdtemp(join(tmpdir(), "mode-log-export-"));
  try {
    const logDir = join(root, "logs");
    await mkdir(logDir, { recursive: true });
    await writeFile(join(logDir, "host-2026-10-09.log"), "host line\n", "utf8");
    await writeFile(join(logDir, "2026-10-08.log"), "old line\n", "utf8");
    const infoLog = join(root, "info.log");
    await writeFile(infoLog, "stdout line\n", "utf8");

    const result = await createLogBundle({
      logDir,
      extraFiles: [infoLog],
      now: new Date("2026-10-27T01:02:03.000Z"),
    });
    assert.ok(result);
    assert.equal(result.fileName, "Mode-logs-20261027-010203.log");
    assert.match(result.bundle, /host line/u);
    assert.match(result.bundle, /stdout line/u);
    assert.match(result.bundle, /omittedFile: 2026-10-08\.log/u);

    const empty = await createLogBundle({ logDir: join(root, "missing") });
    assert.equal(empty, null);
    assert.equal(defaultLogExportFileName(new Date("2026-10-27T01:02:03.000Z")), result.fileName);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
