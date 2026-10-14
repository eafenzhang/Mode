// 汇总覆盖前所有与 conversation-mirror 相关的文件改动，并与当前树对比（只读）。
import { DatabaseSync } from "node:sqlite";
import { existsSync, statSync } from "node:fs";

const CLOBBER = Date.parse("2026-10-13T18:42:36Z");
const db = new DatabaseSync(process.env.HOME + "/.mode/cli/db/db.sqlite", { readOnly: true });
const rows = db
  .prepare(`SELECT id, session_id, time_created, data FROM part WHERE data LIKE '%"tool":"Edit"%' OR data LIKE '%"tool":"Write"%'`)
  .all();

const ops = [];
const constHits = [];
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
  const blob = `${input.old_string ?? ""}\n${input.new_string ?? input.content ?? ""}`;
  if (blob.includes("CONVERSATION_MIRROR_DIR_NAME")) {
    constHits.push({ fp, ts: Number(r.time_created), session: r.session_id });
  }
  const ts = Number(r.time_created);
  if (ts >= CLOBBER) continue;
  const rel = fp.split("\\").join("/").replace("D:/ZCodium/", "");
  if (!/mirror/i.test(rel) && !/ConversationMirror/.test(blob)) continue;
  ops.push({ ts, rel, tool: obj.tool, old: input.old_string, neu: input.new_string ?? input.content });
}
ops.sort((a, b) => a.ts - b.ts);

const byFile = new Map();
for (const o of ops) {
  if (!byFile.has(o.rel)) byFile.set(o.rel, []);
  byFile.get(o.rel).push(o);
}
console.log("文件 | 改动数 | 最晚 | 树中状态");
console.log("-".repeat(110));
for (const [rel, list] of [...byFile].sort()) {
  const last = new Date(list[list.length - 1].ts).toISOString();
  let state;
  try {
    const size = statSync(rel).size;
    state = `存在 ${size}B`;
  } catch {
    state = "★缺失★";
  }
  console.log(`${rel}  | ${list.length} | ${last} | ${state}`);
}

console.log(`\n=== 提到 CONVERSATION_MIRROR_DIR_NAME 的改动（${constHits.length} 条，含覆盖后）===`);
for (const h of [...constHits].sort((a, b) => a.ts - b.ts)) {
  console.log(`${new Date(h.ts).toISOString()}  ${h.ts < CLOBBER ? "覆盖前" : "覆盖后"}  ${h.fp}  ${h.session}`);
}
