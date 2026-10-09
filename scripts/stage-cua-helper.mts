// CUA helper stage 脚本：把 `pnpm build:cua-helper` 的产物三件（entry.cjs /
// cua_ax.node / runtime-manifest.json）原样复制进桌面打包目录
// packages/desktop/bundled-tools/win32-x64/cua-helper。
//
// 打包侧由 electron-builder extraResources 的 win32-only 条目把该目录带进
// resources/tools/cua-helper，打包态 resolvePackagedRuntime 再按 manifest 逐件
// sha256 校验——因此 stage 前先就地校验源产物与 manifest 一致，缺件/坏件时
// 直接失败并指向自建命令，不留下半成品目录。
//
// 幂等：纯内容复制，二次运行后目标三件字节与 manifest sha 不变。
// 运行：仓库根 `pnpm stage:cua-helper`（`pnpm build:cua-helper` 已串联本脚本）。
//
// 扩展名说明：语言政策（docs/specs/language-policy.md）新代码只允许 TypeScript
// 等，禁入 .py/.cs/.js/.mjs/.cjs —— 故本脚本用 .mts（Node 24 type-stripping 原生可跑）。
import { createHash } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
// stage 目标固定 win32-x64：helper manifest 硬编码 platform=win32 / arch=x64，
// 与打包 config 的 win32 条目、build 脚本默认三元组保持同一落点。
const DEFAULT_SOURCE_DIR = join(repoRoot, "packages", "mode-cua", "dist-cua-helper");
const DEFAULT_TARGET_DIR = join(
  repoRoot,
  "packages",
  "desktop",
  "bundled-tools",
  "win32-x64",
  "cua-helper",
);
const MANIFEST_NAME = "runtime-manifest.json";
const ENTRY_NAME = "entry.cjs";
const ADDON_NAME = "cua_ax.node";
const STAGED_FILES = [ENTRY_NAME, ADDON_NAME, MANIFEST_NAME];

export interface StageCuaHelperOptions {
  sourceDir?: string;
  targetDir?: string;
}

export interface StageCuaHelperResult {
  targetDir: string;
  files: string[];
  manifestSha256: string;
}

function sha256File(filePath: string): string {
  return createHash("sha256").update(readFileSync(filePath)).digest("hex");
}

function fail(message: string): never {
  throw new Error(`[stage-cua-helper] ${message}`);
}

function readStagedManifest(sourceDir: string): {
  entry: string;
  addon: string;
  sha256: { entry: string; addon: string };
} {
  const manifestPath = join(sourceDir, MANIFEST_NAME);
  if (!existsSync(manifestPath)) {
    // dev 与 CI 的自建入口只有一条：build 产 dist，stage 再落 bundled-tools。
    fail(`dist 缺少 ${MANIFEST_NAME}（未构建或路径错误）：先运行 pnpm build:cua-helper`);
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(readFileSync(manifestPath, "utf8"));
  } catch {
    fail(`${MANIFEST_NAME} 不是合法 JSON：重新运行 pnpm build:cua-helper 重建`);
  }
  const manifest = parsed as Partial<{
    entry: string;
    addon: string;
    sha256: { entry: string; addon: string };
  }>;
  if (
    manifest.entry !== ENTRY_NAME ||
    manifest.addon !== ADDON_NAME ||
    typeof manifest.sha256?.entry !== "string" ||
    typeof manifest.sha256?.addon !== "string"
  ) {
    fail(
      `${MANIFEST_NAME} 与 helper 契约不符（entry/addon/sha256）：重新运行 pnpm build:cua-helper`,
    );
  }
  return { entry: manifest.entry, addon: manifest.addon, sha256: manifest.sha256 };
}

/**
 * 把 dist-cua-helper 三件套复制到桌面打包目录。幂等：二次运行结果字节一致。
 * 校验全部通过后才创建目标目录，失败不留半成品。
 */
export function stageCuaHelper(options: StageCuaHelperOptions = {}): StageCuaHelperResult {
  const sourceDir = options.sourceDir ?? DEFAULT_SOURCE_DIR;
  const targetDir = options.targetDir ?? DEFAULT_TARGET_DIR;

  const manifest = readStagedManifest(sourceDir);
  for (const fileName of STAGED_FILES) {
    if (!existsSync(join(sourceDir, fileName))) {
      fail(`dist 缺少 ${fileName}：先运行 pnpm build:cua-helper`);
    }
  }
  // 就地校验源产物完整性：dist 可能被外部改动或来自中断的构建，
  // 坏件一旦 stage 进打包目录，要到打包态 resolvePackagedRuntime 才会炸。
  if (sha256File(join(sourceDir, ENTRY_NAME)) !== manifest.sha256.entry) {
    fail(`${ENTRY_NAME} 与 manifest sha256 不一致：重新运行 pnpm build:cua-helper`);
  }
  if (sha256File(join(sourceDir, ADDON_NAME)) !== manifest.sha256.addon) {
    fail(`${ADDON_NAME} 与 manifest sha256 不一致：重新运行 pnpm build:cua-helper`);
  }

  mkdirSync(targetDir, { recursive: true });
  for (const fileName of STAGED_FILES) {
    copyFileSync(join(sourceDir, fileName), join(targetDir, fileName));
  }
  return {
    targetDir,
    files: readdirSync(targetDir).filter((fileName) => STAGED_FILES.includes(fileName)),
    manifestSha256: sha256File(join(targetDir, MANIFEST_NAME)),
  };
}

function isDirectInvocation(): boolean {
  if (!process.argv[1]) return false;
  const invokedPath = resolve(process.argv[1]);
  const modulePath = fileURLToPath(import.meta.url);
  // Windows 文件名大小写不敏感但字符串比较敏感：只比 href/原串会在 CI 大小写差异下
  // 静默跳过 stage（测试全绿但打包缺件），这里按平台降级比较保证直跑必然执行。
  return process.platform === "win32"
    ? invokedPath.toLowerCase() === modulePath.toLowerCase()
    : invokedPath === modulePath;
}

// CLI 入口：被测试 import 时不执行，仅导出 stageCuaHelper。
if (isDirectInvocation()) {
  try {
    const result = stageCuaHelper();
    console.log(
      `[stage-cua-helper] staged ${result.files.length} files → ${relative(repoRoot, result.targetDir).replaceAll("\\", "/")}`,
    );
    console.log(`[stage-cua-helper] sha256 ${MANIFEST_NAME}: ${result.manifestSha256}`);
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
