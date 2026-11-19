// 文件快照（spec §6 第二层）：workspace 递归列文件，跳过构建/评测目录；内容取 sha256。
// 只比内容，不比 mtime/权限。
import { createHash } from "node:crypto";
import { readdir, readFile } from "node:fs/promises";
import { join, relative, sep } from "node:path";

const SKIPPED_DIRS = new Set([".git", "node_modules", ".mode-eval"]);

export async function takeFileSnapshot(root: string): Promise<Record<string, string>> {
  const files: Record<string, string> = {};
  await walk(root, root, files);
  return files;
}

async function walk(root: string, current: string, files: Record<string, string>): Promise<void> {
  const entries = await readdir(current, { withFileTypes: true });
  for (const entry of entries) {
    const path = join(current, entry.name);
    if (entry.isDirectory()) {
      if (SKIPPED_DIRS.has(entry.name)) continue;
      await walk(root, path, files);
      continue;
    }
    if (!entry.isFile()) continue;
    const content = await readFile(path);
    const relativePath = relative(root, path).split(sep).join("/");
    files[relativePath] = createHash("sha256").update(content).digest("hex");
  }
}
