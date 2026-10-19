// Tauri updater 的 `latest.json` 生成器（M5 Task 1）。
//
// 为什么需要它：`tauri build` 在 `bundle.createUpdaterArtifacts = true` 且设置了
// `TAURI_SIGNING_PRIVATE_KEY` 时只产出安装包与其 `.sig`（本机实测：NSIS 目录里没有
// `latest.json`）。`latest.json` 是 updater 的**清单**，必须由发布流程显式生成：
// 版本、发布时间、平台键、签名、下载 URL 五项缺一不可。
//
// 用法：
//   node scripts/tauri-update-manifest.ts \
//     --version 0.0.2 \
//     --artifact "dist/tauri/Mode-0.0.2-win-x64.exe" \
//     --signature "dist/tauri/Mode-0.0.2-win-x64.exe.sig" \
//     --url "https://github.com/eafenzhang/Mode/releases/download/v0.0.2/Mode-0.0.2-win-x64.exe" \
//     --output "dist/tauri/latest.json"
//
// 可选：`--notes "<text>"`、`--pub-date "<RFC3339>"`（默认当前 UTC）。
//
// 纪律：
// - `signature` 是 `.sig` 文件的**原文**（插件会先 base64 解码它）；不要在这里二次编码。
// - URL 必须是 https：updater 在 release 构建里拒绝非 https 的 endpoint。
// - 只写一个平台条目（Windows x64）；macOS/Linux 未验证，不伪造条目（DR-M5-2）。
import { readFileSync, writeFileSync } from "node:fs";

/** Tauri updater 在 Windows x64 上查的 manifest 键（`tauri_plugin_updater::target()`）。 */
const WINDOWS_X86_64 = "windows-x86_64";

interface Options {
  version: string;
  artifact: string;
  signature: string;
  url: string;
  output: string;
  notes: string;
  pubDate: string;
}

function fail(message: string): never {
  console.error(`[tauri-update-manifest] ${message}`);
  process.exit(1);
}

function parseArgs(argv: string[]): Options {
  const values = new Map<string, string>();
  for (let i = 0; i < argv.length; i += 1) {
    const key = argv[i];
    if (!key.startsWith("--")) fail(`无法识别的参数：${key}`);
    const value = argv[i + 1];
    if (value === undefined || value.startsWith("--")) fail(`${key} 缺少取值`);
    values.set(key.slice(2), value);
    i += 1;
  }

  const required = (name: string): string => {
    const value = values.get(name)?.trim();
    if (!value) fail(`缺少必需参数 --${name}`);
    return value;
  };

  return {
    version: required("version"),
    artifact: required("artifact"),
    signature: required("signature"),
    url: required("url"),
    output: required("output"),
    notes: values.get("notes") ?? "",
    pubDate: values.get("pub-date")?.trim() || new Date().toISOString(),
  };
}

function main(): void {
  const options = parseArgs(process.argv.slice(2));

  if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/u.test(options.version)) {
    fail(`--version 不是合法版本号：${options.version}`);
  }
  if (!options.url.startsWith("https://")) {
    fail(`--url 必须是 https（release 构建会拒绝 http endpoint）：${options.url}`);
  }
  // 让 URL 与将要上传的产物同名：发布后文件名是人工排障时唯一的锚点。
  const artifactName = options.artifact.replaceAll("\\", "/").split("/").pop() ?? "";
  const urlName = options.url.split("?")[0]?.split("/").pop() ?? "";
  if (!artifactName || artifactName !== urlName) {
    fail(`--url 的文件名必须与 --artifact 一致（artifact=${artifactName}，url=${urlName}）`);
  }

  let signature: string;
  try {
    signature = readFileSync(options.signature, "utf8").trim();
  } catch (error) {
    fail(`读不到 --signature：${String(error)}`);
  }
  if (!signature) fail(`--signature 文件为空：${options.signature}`);
  // `.sig` 是 base64；这里只做形状检查，真正的密码学校验在客户端。
  if (!/^[A-Za-z0-9+/=\r\n]+$/u.test(signature)) {
    fail(`--signature 不是 base64 文本：${options.signature}`);
  }

  const manifest = {
    version: options.version,
    notes: options.notes,
    pub_date: options.pubDate,
    platforms: {
      [WINDOWS_X86_64]: {
        signature,
        url: options.url,
      },
    },
  };

  writeFileSync(options.output, `${JSON.stringify(manifest, null, 2)}\n`, "utf8");
  console.log(
    `[tauri-update-manifest] wrote ${options.output} version=${options.version} platform=${WINDOWS_X86_64}`,
  );
}

main();
