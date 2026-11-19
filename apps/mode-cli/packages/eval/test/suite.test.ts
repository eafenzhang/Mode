// Suite 加载（spec §3/§4）：单文件、目录、行为夹具筛选、重复 id。
import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { loadSuite, SuiteLoadError } from "../src/suite.js";

test("loadSuite 读取单文件 suite", async () => {
  const root = await mkdtemp(join(tmpdir(), "mode-eval-suite-"));
  try {
    const path = join(root, "suite.json");
    await writeFile(
      path,
      JSON.stringify({
        schemaVersion: 1,
        name: "demo",
        cases: [
          {
            case: "a",
            prompt: "p",
            script: [{ text: "ok" }],
          },
        ],
      }),
      "utf8",
    );
    const suite = await loadSuite(path);
    assert.equal(suite.schemaVersion, 1);
    assert.equal(suite.name, "demo");
    assert.equal(suite.cases.length, 1);
    assert.equal(suite.cases[0]?.case, "a");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("loadSuite 目录模式：行为夹具入选、协议夹具跳过", async () => {
  const root = await mkdtemp(join(tmpdir(), "mode-eval-dir-"));
  try {
    await writeFile(
      join(root, "behavior.json"),
      JSON.stringify({ case: "b", prompt: "p", seed: {}, script: [{ text: "ok" }] }),
      "utf8",
    );
    await writeFile(
      join(root, "protocol.json"),
      JSON.stringify({ direction: "host", frame: { method: "x" }, note: "n" }),
      "utf8",
    );
    const suite = await loadSuite(root);
    assert.deepEqual(
      suite.cases.map((entry) => entry.case),
      ["b"],
    );
    assert.equal(suite.name, root.split(/[\\/]/).pop());
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("loadSuite 拒绝重复 case id", async () => {
  const root = await mkdtemp(join(tmpdir(), "mode-eval-dup-"));
  try {
    const path = join(root, "suite.json");
    await writeFile(
      path,
      JSON.stringify({
        schemaVersion: 1,
        cases: [
          { case: "same", prompt: "p", script: [{ text: "a" }] },
          { case: "same", prompt: "q", script: [{ text: "b" }] },
        ],
      }),
      "utf8",
    );
    await assert.rejects(() => loadSuite(path), SuiteLoadError);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("loadSuite 缺 script 抛错", async () => {
  const root = await mkdtemp(join(tmpdir(), "mode-eval-bad-"));
  try {
    const path = join(root, "case.json");
    await writeFile(path, JSON.stringify({ case: "x", prompt: "p" }), "utf8");
    await assert.rejects(() => loadSuite(path));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("空目录没有行为用例时报错", async () => {
  const root = await mkdtemp(join(tmpdir(), "mode-eval-empty-"));
  try {
    await mkdir(root, { recursive: true });
    await assert.rejects(() => loadSuite(root), SuiteLoadError);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
