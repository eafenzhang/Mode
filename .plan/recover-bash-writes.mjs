// 找覆盖前通过 Bash 写入 mirror 相关文件的命令（只读）。
import { DatabaseSync } from "node:sqlite";

const CLOBBER = Date.parse("2026-10-13T18:42:36Z");
const db = new DatabaseSync(process.env.HOME + "/.mode/cli/db/db.sqlite", { readOnly: true });
const rows = db
  .prepare(`SELECT id, session_id, time_created, data FROM part WHERE data LIKE '%"tool":"Bash"%'`)
  .all();
const out = [];
for (const r of rows) {
  const ts = Number(r.time_created);
  if (ts >= CLOBBER) continue;
  let obj;
  try {
    obj = JSON.parse(r.data);
  } catch {
    continue;
  }
  const input = obj?.state?.input ?? obj?.input;
  const cmd = typeof input?.command === "string" ? input.command : "";
  if (!cmd) continue;
  if (!/mirror/i.test(cmd)) continue;
  if (!/(>|cat|tee|patch|sed -i|python)/i.test(cmd)) continue;
  out.push({ ts, session: r.session_id, cmd });
}
out.sort((a, b) => a.ts - b.ts);
console.log(`命中 ${out.length} 条 Bash 写入命令\n`);
for (const e of out) {
  console.log(`${new Date(e.ts).toISOString()}  ${e.session}`);
  console.log(`  ${e.cmd.slice(0, 500).replace(/\n/g, "\n  ")}\n`);
}
