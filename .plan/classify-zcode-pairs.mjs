// 对每对 zcode*/mode* 文件，比较导出的符号集，判断能否安全改名。
import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";

const files = execFileSync("git", ["ls-files"], { maxBuffer: 1 << 28 })
  .toString()
  .split("\n")
  .filter((f) => /\/(zcode)[A-Za-z0-9-]*\.tsx?$/.test(f))
  .filter((f) => !f.includes("/dist/") && !f.includes("/node_modules/"));

const symbols = (f) => {
  let t;
  try {
    t = readFileSync(f, "utf8");
  } catch {
    return null;
  }
  const out = new Set();
  for (const m of t.matchAll(/^export\s+(?:const|function|class|interface|type|enum|async function)\s+([A-Za-z0-9_]+)/gm)) out.add(m[1]);
  for (const m of t.matchAll(/^export\s*\{([^}]+)\}/gm)) {
    for (const part of m[1].split(",")) {
      const name = part.trim().split(/\s+as\s+/).pop().trim();
      if (name) out.add(name);
    }
  }
  return out;
};

const same = [];
const differ = [];
for (const z of files) {
  const m = z.replace(/\/(zcode)([A-Za-z0-9-]*)\.(tsx?)$/, "/mode$2.$3");
  let mt;
  try {
    mt = readFileSync(m, "utf8");
  } catch {
    continue;
  }
  const zs = symbols(z);
  const ms = symbols(m);
  if (!zs || !ms) continue;
  const onlyZ = [...zs].filter((s) => !ms.has(s));
  if (onlyZ.length === 0) same.push(z);
  else differ.push([z, onlyZ.length, onlyZ.slice(0, 3).join(",")]);
}

console.log(`=== 符号集一致（可安全删旧文件+改说明符）${same.length} ===`);
for (const s of same) console.log(`  ${s}`);
console.log(`\n=== 符号集不同（需连符号一起改）${differ.length} ===`);
for (const [z, n, sample] of differ) console.log(`  ${z}  仅旧文件有 ${n} 个: ${sample}...`);
