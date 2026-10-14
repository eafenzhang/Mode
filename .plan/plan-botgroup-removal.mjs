// 计算「同步引入、产品零引用」的 bot-group 模块及其专属测试的删除集。
import { execFileSync } from "node:child_process";
import { readFileSync, existsSync } from "node:fs";

const BASE = "70a2f9e4";
const list = execFileSync("git", ["ls-files"], { maxBuffer: 1 << 28 }).toString().split("\n").filter(Boolean);
const tsFiles = list.filter((f) => /\.(ts|tsx)$/.test(f) && !f.includes("/dist/") && !f.includes("/node_modules/"));
const inBase = (f) => {
  try {
    execFileSync("git", ["cat-file", "-e", `${BASE}:${f}`], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
};

// 候选：services/src/bots 下「基线里没有」的文件
const candidates = tsFiles.filter((f) => f.startsWith("packages/services/src/bots/") && !inBase(f));
console.log(`services/src/bots 下同步新增文件：${candidates.length}`);
for (const c of candidates) console.log(`  ${c}`);

// 引用计数：谁（非候选、非 dist/out）在源码里引用候选的模块名
const names = candidates.map((c) => c.split("/").pop().replace(/\.ts$/, ""));
const srcFiles = tsFiles.filter((f) => !f.startsWith("packages/services/test/") && /\.(ts|tsx)$/.test(f));
const refCount = new Map(names.map((n) => [n, []]));
for (const f of srcFiles) {
  if (candidates.includes(f)) continue;
  let text;
  try {
    text = readFileSync(f, "utf8");
  } catch {
    continue;
  }
  for (const n of names) {
    if (new RegExp(`["'./]${n}(\\.js)?["'/]`).test(text)) refCount.get(n).push(f);
  }
}

console.log("\n=== 产品源码引用（应全为空才可删）===");
for (const [n, refs] of refCount) {
  if (refs.length) console.log(`  ✗ ${n} <- ${refs.slice(0, 4).join(", ")}`);
}
console.log("（以上为空 = 无产品引用）");

// 专属测试：只引用「无产品引用」的候选模块的测试文件
const deletable = names.filter((n) => refCount.get(n).length === 0);
const testFiles = list.filter((f) => /\/test\//.test(f) && /\.(ts|tsx|mts)$/.test(f) && !f.includes("/dist/"));
const testToDelete = [];
for (const t of testFiles) {
  let text;
  try {
    text = readFileSync(t, "utf8");
  } catch {
    continue;
  }
  const imports = deletable.filter((n) => new RegExp(`["'./]${n}(\\.js)?["']`).test(text));
  if (imports.length > 0) testToDelete.push({ t, imports });
}
console.log(`\n=== 待删测试（引用将被删模块）${testToDelete.length} ===`);
for (const { t, imports } of testToDelete) console.log(`  ${t}  <- ${imports.join(", ")}  (基线${inBase(t) ? "有" : "无"})`);
console.log(`\n=== 待删模块 ${deletable.length} ===`);
for (const n of deletable) console.log(`  ${candidates[names.indexOf(n)]}`);
