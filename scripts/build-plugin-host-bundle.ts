// 构建插件宿主 sidecar 的单文件 bundle + 资产清单（M3 生态域 1 · Task 1/6）。
//
// 用法：
//   pnpm exec tsx scripts/build-plugin-host-bundle.ts
//
// 产物（不提交）：dist/plugin-host/plugin-host.mjs + dist/plugin-host/manifest.json
// manifest 形状与 crates/plugin-host/src/manifest.rs 的 PluginHostManifest 一致。
// assetUrl 默认是相对文件名；上传 CDN 时用 MODE_PLUGIN_HOST_ASSET_URL 覆盖。
//
// `buildPluginHostBundle` 导出给测试复用（注入 outDir/entry/version/platform/assetUrl），
// 校验契约见 docs/specs/plugin-host.md §5.9。

import { build } from "esbuild";
import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = fileURLToPath(new URL("../", import.meta.url));
const DEFAULT_OUT_DIR = join(ROOT, "dist", "plugin-host");
const DEFAULT_ENTRY = join(ROOT, "tools", "plugin-host", "src", "main.ts");

export interface BuildPluginHostOptions {
  outDir?: string;
  entry?: string;
  version?: string;
  platform?: string;
  assetUrl?: string;
}

export interface PluginHostManifest {
  schemaVersion: 1;
  version: string;
  platform: string;
  entry: string;
  assetUrl: string;
  sha256: string;
}

export interface BuildPluginHostResult {
  outDir: string;
  entryPath: string;
  bytes: number;
  sha256: string;
  manifest: PluginHostManifest;
}

export async function buildPluginHostBundle(
  options: BuildPluginHostOptions = {},
): Promise<BuildPluginHostResult> {
  const outDir = options.outDir ?? DEFAULT_OUT_DIR;
  const entry = options.entry ?? DEFAULT_ENTRY;
  const entryPath = join(outDir, "plugin-host.mjs");

  await mkdir(outDir, { recursive: true });
  await build({
    entryPoints: [entry],
    outfile: entryPath,
    bundle: true,
    platform: "node",
    format: "esm",
    target: "node24",
    legalComments: "none",
    sourcemap: false,
  });

  const bytes = await readFile(entryPath);
  const sha256 = createHash("sha256").update(bytes).digest("hex");
  const version =
    options.version?.trim() || process.env.MODE_PLUGIN_HOST_VERSION?.trim() || "0.1.0";
  const platform =
    options.platform ??
    (process.platform === "win32"
      ? `win32-${process.arch === "x64" ? "x64" : process.arch}`
      : `${process.platform}-${process.arch}`);
  const assetUrl =
    options.assetUrl?.trim() ||
    process.env.MODE_PLUGIN_HOST_ASSET_URL?.trim() ||
    "plugin-host.mjs";
  const manifest: PluginHostManifest = {
    schemaVersion: 1,
    version,
    platform,
    entry: "plugin-host.mjs",
    assetUrl,
    sha256,
  };
  await writeFile(
    join(outDir, "manifest.json"),
    `${JSON.stringify(manifest, null, 2)}\n`,
    "utf8",
  );
  return { outDir, entryPath, bytes: bytes.length, sha256, manifest };
}

const invokedPath = process.argv[1] ? resolve(process.argv[1]) : "";
if (invokedPath === fileURLToPath(import.meta.url)) {
  const result = await buildPluginHostBundle();
  process.stdout.write(
    `wrote ${result.outDir} (${result.bytes} bytes, sha256 ${result.sha256})\n`,
  );
}
