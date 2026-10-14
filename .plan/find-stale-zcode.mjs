// 找出所有「有 mode* 对应实现」的 zcode* 残留源文件，并检查是否被非残留文件引用。
import { readFileSync, existsSync } from "node:fs";
import { execFileSync } from "node:child_process";

const files = execFileSync("git", ["ls-files"], { maxBuffer: 1 << 28 })
  .toString()
  .split("\n")
  .filter((f) => /\/(zcode)[A-Za-z0-9-]*\.tsx?$/.test(f))
  .filter((f) => !f.includes("/dist/") && !f.includes("/node_modules/"));

const counterpart = (f) => f.replace(/\/(zcode)([A-Za-z0-9-]*)\.(tsx?)$/, "/mode$2.$3");

const candidates = [];
for (const f of files) {
  const m = counterpart(f);
  if (existsSync(m)) candidates.push({ zcode: f, mode: m });
}

console.log(`有 mode* 对应的 zcode* 文件：${candidates.length}\n`);
for (const c of candidates) console.log(`  ${c.zcode}\n    -> ${c.mode}`);

// 谁引用了这些残留文件（按模块说明符或路径片段）
const all = execFileSync("git", ["ls-files"], { maxBuffer: 1 << 28 }).toString().split("\n");
const staleSet = new Set(candidates.map((c) => c.zcode));
const importers = new Map();
for (const file of all) {
  if (!/\.(ts|tsx)$/.test(file) || file.includes("/dist/")) continue;
  let text;
  try {
    text = readFileSync(file, "utf8");
  } catch {
    continue;
  }
  for (const c of candidates) {
    const base = c.zcode.split("/").pop().replace(/\.tsx?$/, "");
    if (!text.includes(base)) continue;
    if (staleSet.has(file)) continue;
    if (!importers.has(c.zcode)) importers.set(c.zcode, new Set());
    importers.get(c.zcode).add(file);
  }
}
console.log("\n=== 被非残留文件引用的（不能直接删）===");
if (importers.size === 0) console.log("  （无）");
for (const [k, v] of importers) console.log(`  ${k}\n     <- ${[...v].join(", ")}`);
