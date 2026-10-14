// 按 Write/Edit 时序回放，重建某个文件在覆盖前的最终内容（只读 DB）。
import { DatabaseSync } from "node:sqlite";

const CLOBBER = Date.parse("2026-10-13T18:42:36Z");
const needle = (process.argv[2] ?? "").toLowerCase();
const show = process.argv.includes("--show");

const db = new DatabaseSync(process.env.HOME + "/.mode/cli/db/db.sqlite", { readOnly: true });
const rows = db
  .prepare(
    `SELECT id, session_id, time_created, data FROM part
     WHERE data LIKE '%"tool":"Edit"%' OR data LIKE '%"tool":"Write"%'`,
  )
  .all();

const ops = [];
for (const r of rows) {
  let obj;
  try {
    obj = JSON.parse(r.data);
  } catch {
    continue;
  }
  const input = obj?.state?.input ?? obj?.input;
  const fp = input?.file_path;
  if (typeof fp !== "string") continue;
  if (needle && !fp.toLowerCase().includes(needle)) continue;
  const ts = Number(r.time_created);
  if (ts >= CLOBBER) continue;
  ops.push({ ts, fp: fp.split("\\").join("/"), tool: obj.tool, old: input.old_string, neu: input.new_string ?? input.content });
}
ops.sort((a, b) => a.ts - b.ts);

// 分组：同一文件的内容可能来自 Write（全量）或 Edit（增量）
const byFile = new Map();
for (const o of ops) {
  if (!byFile.has(o.fp)) byFile.set(o.fp, []);
  byFile.get(o.fp).push(o);
}

for (const [fp, list] of byFile) {
  console.log(`\n${"=".repeat(80)}\n${fp}   ops=${list.length}`);
  let cur = null;
  const problems = [];
  for (const o of list) {
    const stamp = new Date(o.ts).toISOString();
    if (o.tool === "Write") {
      cur = o.neu;
      console.log(`  ${stamp}  Write   ${(o.neu ?? "").length} bytes  (基线重置)`);
      continue;
    }
    if (cur === null) {
      // 没有基线 Write：只能标记，稍后靠「当前文件」当基线
      problems.push(`${stamp} Edit 前无 Write 基线`);
      continue;
    }
    const idx = cur.indexOf(o.old);
    if (idx < 0) {
      problems.push(`${stamp} Edit 的 old_string 未命中`);
      continue;
    }
    cur = cur.slice(0, idx) + o.neu + cur.slice(idx + o.old.length);
    console.log(`  ${stamp}  Edit    ok  (+${o.neu.split("\n").length - o.old.split("\n").length})`);
  }
  for (const p of problems) console.log(`  ⚠ ${p}`);
  if (cur !== null) {
    const out = `D:/ZCodium/.plan/recovered/${fp.replace("D:/ZCodium/", "").split("/").join("__")}`;
    const fs = await import("node:fs");
    fs.writeFileSync(out, cur);
    console.log(`  → 重建 ${cur.split("\n").length} 行 / ${cur.length} 字节，写入 ${out}`);
    if (show) console.log(cur);
  }
}
