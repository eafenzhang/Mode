// 构建插件宿主 sidecar 的单文件 bundle + 资产清单（M3 生态域 1 · Task 1/6）。
//
// 用法：
//   pnpm exec tsx scripts/build-plugin-host-bundle.ts
//
// 产物（不提交）：dist/plugin-host/plugin-host.mjs + dist/plugin-host/manifest.json
// manifest 形状与 crates/plugin-host/src/manifest.rs 的 PluginHostManifest 一致。
// assetUrl 默认是相对文件名；上传 CDN 时用 MODE_PLUGIN_HOST_ASSET_URL 覆盖。
import { build } from "esbuild";
import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = fileURLToPath(new URL("../", import.meta.url));
const OUT_DIR = join(ROOT, "dist", "plugin-host");
const ENTRY = join(ROOT, "tools", "plugin-host", "src", "main.ts");

await mkdir(OUT_DIR, { recursive: true });
await build({
  entryPoints: [ENTRY],
  outfile: join(OUT_DIR, "plugin-host.mjs"),
  bundle: true,
  platform: "node",
  format: "esm",
  target: "node24",
  legalComments: "none",
  sourcemap: false,
});

const bytes = await readFile(join(OUT_DIR, "plugin-host.mjs"));
const sha256 = createHash("sha256").update(bytes).digest("hex");
const version = process.env.MODE_PLUGIN_HOST_VERSION?.trim() || "0.1.0";
const platform =
  process.platform === "win32"
    ? `win32-${process.arch === "x64" ? "x64" : process.arch}`
    : `${process.platform}-${process.arch}`;
const manifest = {
  schemaVersion: 1,
  version,
  platform,
  entry: "plugin-host.mjs",
  assetUrl: process.env.MODE_PLUGIN_HOST_ASSET_URL?.trim() || "plugin-host.mjs",
  sha256,
};
await writeFile(
  join(OUT_DIR, "manifest.json"),
  `${JSON.stringify(manifest, null, 2)}\n`,
  "utf8",
);
process.stdout.write(
  `wrote ${OUT_DIR} (${bytes.length} bytes, sha256 ${sha256})\n`,
);
