// 找出覆盖前对 conversation-mirror 相关新文件的写入（只读）。
import { DatabaseSync } from "node:sqlite";

const CLOBBER = Date.parse("2026-10-13T18:42:36Z");
const needle = process.argv[2] ?? "conversation-mirror";
const onlyBefore = !process.argv.includes("--all");

const db = new DatabaseSync(process.env.HOME + "/.mode/cli/db/db.sqlite", { readOnly: true });
const rows = db
  .prepare(
    `SELECT id, session_id, time_created, data FROM part
     WHERE data LIKE '%"tool":"Edit"%' OR data LIKE '%"tool":"Write"%'`,
  )
  .all();

const out = [];
for (const r of rows) {
  let obj;
  try {
    obj = JSON.parse(r.data);
  } catch {
    continue;
  }
  const input = obj?.state?.input ?? obj?.input;
  const fp = input?.file_path;
  if (typeof fp !== "string" || !fp.toLowerCase().includes(needle)) continue;
  const ts = Number(r.time_created);
  if (onlyBefore && ts >= CLOBBER) continue;
  out.push({
    ts,
    session: r.session_id,
    tool: obj.tool,
    fp,
    old: input.old_string,
    neu: input.new_string ?? input.content,
  });
}
out.sort((a, b) => a.ts - b.ts);
console.log(`共 ${out.length} 条\n`);
for (const e of out) {
  console.log(`${new Date(e.ts).toISOString()}  ${e.tool}  ${e.fp}  old=${e.old === undefined ? "-" : e.old.length}  new=${(e.neu ?? "").length}  ${e.session}`);
}
