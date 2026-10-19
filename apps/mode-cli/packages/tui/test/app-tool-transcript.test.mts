import assert from "node:assert/strict";
import test from "node:test";

import { buildToolTranscriptProjection } from "../src/app-tool-transcript.js";

test("工具投影：Read 标题取文件名，offset/limit 逐行展示", () => {
  assert.deepEqual(
    buildToolTranscriptProjection("Read", { file_path: "/w/a.ts", offset: 10, limit: 20 }, "/w"),
    {
      detailLines: ["offset: 10", "limit: 20"],
      title: "Read a.ts",
    },
  );
  assert.deepEqual(
    buildToolTranscriptProjection("Read", { file_path: "/w/a.ts", offset: 0, limit: 0 }, "/w"),
    {
      detailLines: ["offset: 0", "limit: 0"],
      title: "Read a.ts",
    },
  );
  assert.deepEqual(buildToolTranscriptProjection("Read", { file_path: "/w/a.ts" }), {
    detailLines: [],
    title: "Read /w/a.ts",
  });
});

test("工具投影：Bash 只展示 command，空输入无细节行", () => {
  assert.deepEqual(
    buildToolTranscriptProjection("Bash", { command: "ls -la", description: "list" }),
    { detailLines: ["command: ls -la"] },
  );
  assert.deepEqual(buildToolTranscriptProjection("Bash", {}), { detailLines: [] });
});

test("工具投影：Write 展示路径，Edit 只给标题", () => {
  assert.deepEqual(buildToolTranscriptProjection("Write", { file_path: "/w/a.ts" }, "/w"), {
    detailLines: ["file_path: /w/a.ts"],
  });
  assert.deepEqual(buildToolTranscriptProjection("Edit", { file_path: "/w/a.ts" }, "/w"), {
    detailLines: [],
    title: "Edit a.ts",
  });
  assert.deepEqual(buildToolTranscriptProjection("Edit", { file_path: "/w/a.ts" }), {
    detailLines: [],
    title: "Edit /w/a.ts",
  });
});

test("工具投影：Grep/Glob 展示 pattern 与可选 path", () => {
  assert.deepEqual(
    buildToolTranscriptProjection("Grep", { pattern: "foo", path: "/w/src" }, "/w"),
    { detailLines: ["pattern: foo", "path: /w/src"] },
  );
  assert.deepEqual(buildToolTranscriptProjection("Grep", { pattern: "foo" }), {
    detailLines: ["pattern: foo"],
  });
  assert.deepEqual(
    buildToolTranscriptProjection("Glob", { pattern: "**/*.ts", path: "/w/src" }, "/w"),
    { detailLines: ["pattern: **/*.ts", "path: /w/src"] },
  );
});

test("工具投影：未知工具按字段类型概括展示", () => {
  assert.deepEqual(buildToolTranscriptProjection("Custom", { a: 1 }), {
    detailLines: ["a: 1"],
  });
  assert.deepEqual(
    buildToolTranscriptProjection("Custom", {
      nested: { b: 1 },
      list: [1, 2],
      flag: true,
      none: null,
    }),
    {
      detailLines: ["nested: {1 keys}", "list: [2 items]", "flag: true", "none: null"],
    },
  );
  assert.deepEqual(buildToolTranscriptProjection("Custom", {}), { detailLines: [] });
});
