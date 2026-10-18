/**
 * M3 Task 6 补充：plugin-host bundle 与 manifest 的契约校验（spec §5.9）。
 *
 * 只验证构建产物本身：manifest 字段、sha256 与 entry 一致、bundle 能被 node 启动并
 * 按 NDJSON 协议应答 health/listTools（隔离的空数据根 → servers=0、tools=[]）。
 * 不覆盖 CDN 上传与真实 assetUrl 部署。
 */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { buildPluginHostBundle } from "../../../scripts/build-plugin-host-bundle.ts";

async function withBundle<T>(
  run: (context: { outDir: string; entryPath: string; dataRoot: string }) => Promise<T>,
): Promise<T> {
  const outDir = await mkdtemp(join(tmpdir(), "mode-plugin-host-bundle-"));
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-plugin-host-data-"));
  try {
    const result = await buildPluginHostBundle({ outDir });
    return await run({ outDir, entryPath: result.entryPath, dataRoot });
  } finally {
    await rm(outDir, { recursive: true, force: true });
    await rm(dataRoot, { recursive: true, force: true });
  }
}

function nextFrame(child: ReturnType<typeof spawn>, id: number): Promise<Record<string, unknown>> {
  return new Promise((resolve, reject) => {
    let buffer = "";
    const timer = setTimeout(() => {
      cleanup();
      reject(new Error(`等待 id=${id} 的应答超时`));
    }, 15_000);
    const cleanup = (): void => {
      clearTimeout(timer);
      child.stdout?.off("data", onData);
    };
    const onData = (chunk: Buffer): void => {
      buffer += chunk.toString("utf8");
      for (;;) {
        const newline = buffer.indexOf("\n");
        if (newline === -1) {
          return;
        }
        const line = buffer.slice(0, newline).trim();
        buffer = buffer.slice(newline + 1);
        if (!line) {
          continue;
        }
        const frame = JSON.parse(line) as Record<string, unknown>;
        if (frame.id === id) {
          cleanup();
          resolve(frame);
          return;
        }
      }
    };
    child.stdout?.on("data", onData);
  });
}

test("manifest 字段与 bundle 字节一致", async () => {
  await withBundle(async ({ outDir, entryPath }) => {
    const raw = JSON.parse(await readFile(join(outDir, "manifest.json"), "utf8")) as {
      schemaVersion: number;
      version: string;
      platform: string;
      entry: string;
      assetUrl: string;
      sha256: string;
    };
    assert.equal(raw.schemaVersion, 1);
    assert.equal(raw.version, "0.1.0");
    assert.equal(raw.platform.length > 0, true);
    assert.equal(raw.entry, "plugin-host.mjs");
    assert.equal(raw.assetUrl, "plugin-host.mjs");

    const bytes = await readFile(entryPath);
    assert.equal(bytes.length > 0, true);
    assert.equal(raw.sha256, createHash("sha256").update(bytes).digest("hex"));
  });
});

test("version/platform/assetUrl 可注入（CDN 形态）", async () => {
  const outDir = await mkdtemp(join(tmpdir(), "mode-plugin-host-manifest-"));
  try {
    const result = await buildPluginHostBundle({
      outDir,
      version: "9.9.9",
      platform: "test-arch",
      assetUrl: "https://cdn.example/plugin-host.mjs",
    });
    assert.equal(result.manifest.version, "9.9.9");
    assert.equal(result.manifest.platform, "test-arch");
    assert.equal(result.manifest.assetUrl, "https://cdn.example/plugin-host.mjs");
  } finally {
    await rm(outDir, { recursive: true, force: true });
  }
});

test("空数据根下 bundle 应答 health 与 listTools", async () => {
  await withBundle(async ({ entryPath, dataRoot }) => {
    const child = spawn(process.execPath, [entryPath], {
      env: { ...process.env, MODE_DATA_BASE_DIR: dataRoot },
      stdio: ["pipe", "pipe", "pipe"],
    });
    try {
      child.stdin?.write(`${JSON.stringify({ id: 1, method: "plugin/health", params: {} })}\n`);
      const health = await nextFrame(child, 1);
      assert.deepEqual(health.result, { ok: true, servers: 0 });

      child.stdin?.write(`${JSON.stringify({ id: 2, method: "plugin/listTools", params: {} })}\n`);
      const listed = await nextFrame(child, 2);
      assert.deepEqual(listed.result, { tools: [] });
    } finally {
      child.kill();
      await new Promise((resolve) => child.once("close", resolve));
    }
  });
});
