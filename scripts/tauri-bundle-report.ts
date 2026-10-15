// Tauri 安装包的报告器（路线图 M1 Task 5）：算 SHA-256、记体积、打印校验行。
//
// 为什么是一个脚本而不是 CI 里的 `node -e`：这段逻辑有三件必须一致的事——
// 校验行格式（要能被 checksums job 的合并清单直接吃）、体积口径（三平台同一算法）、
// 以及落盘留痕（体积是三指标之一，路线图「度量与预算」）。写成一次性内联字符串，
// 三平台腿各有一份副本，改一处必漏两处。
//
// 用法：node scripts/tauri-bundle-report.ts <安装包路径>
// 需要 pnpm workspace 的 Node（类型擦除），CI 用同一套 Node 24。
import { createHash } from "node:crypto";
import { appendFileSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";

/**
 * 校验行的文件名部分。
 *
 * 与 `checksums` job 的合并清单同一约定：`<sha256>  <文件名>`（两个空格），
 * 且**只写 basename**——合并清单是在 Release 根目录下按文件名索引的。
 */
function checksumLine(file: string): { line: string; sizeMb: string } {
  const bytes = readFileSync(file);
  const digest = createHash("sha256").update(bytes).digest("hex");
  // MiB 口径统一：三平台安装包按解包前的字节数比，与路线图「体积口径」一致。
  const sizeMb = (statSync(file).size / 1024 / 1024).toFixed(2);
  return { line: `${digest}  ${basename(file)}`, sizeMb };
}

function main(): void {
  const target = process.argv[2];
  if (!target) {
    console.error("[tauri-bundle-report] 用法：node scripts/tauri-bundle-report.ts <安装包路径>");
    process.exit(1);
  }

  const { line, sizeMb } = checksumLine(target);
  writeFileSync(`${target}.sha256`, `${line}\n`);

  // 体积留痕：一份落进构建产物目录（连同 artifacts 一起上传，便于事后比对），
  // 一份进 GitHub 的 job summary（跑到哪一版、多大，打开运行页就能看到）。
  const sizeDir = dirname(target);
  appendFileSync(join(sizeDir, "size.txt"), `${basename(target)}  ${sizeMb} MiB\n`);
  if (process.env.GITHUB_STEP_SUMMARY) {
    appendFileSync(process.env.GITHUB_STEP_SUMMARY, `- \`${basename(target)}\`: ${sizeMb} MiB\n`);
  }

  console.log(line);
}

main();
