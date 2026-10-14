// 从会话数据库回放覆盖前的编辑并应用到工作树（行尾归一后匹配）。
//   node restore-mirror.mjs          干跑
//   node restore-mirror.mjs --apply  写入
import { DatabaseSync } from "node:sqlite";
import { readFileSync, writeFileSync, existsSync } from "node:fs";

const CLOBBER = Date.parse("2026-10-13T18:42:36Z");
// 只重放「覆盖窗口」内的编辑：早于此的都已进过历史提交，重放会重复。
const WINDOW_START = Date.parse("2026-10-13T16:00:00Z");
const APPLY = process.argv.includes("--apply");

const FILES = [
  "packages/shared/src/channels.ts",
  "packages/shared/src/remote-workspace-identity.ts",
  "packages/services/src/index.ts",
  "packages/services/src/node.ts",
  "packages/services/src/paths.ts",
  "packages/services/src/accessor.ts",
  "packages/client/src/remoteServiceAccess.ts",
  "packages/desktop/src/host/remoteWorkspaceServiceCollection.ts",
];

const db = new DatabaseSync(process.env.HOME + "/.mode/cli/db/db.sqlite", { readOnly: true });
const rows = db
  .prepare(`SELECT id, session_id, time_created, data FROM part WHERE data LIKE '%"tool":"Edit"%'`)
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
  const rel = fp.split("\\").join("/").replace("D:/ZCodium/", "");
  if (!FILES.includes(rel)) continue;
  const ts = Number(r.time_created);
  if (ts >= CLOBBER || ts < WINDOW_START) continue;
  ops.push({ ts, rel, old: input.old_string, neu: input.new_string });
}
ops.sort((a, b) => a.ts - b.ts);

// 去重：同一 (file, old, new) 只保留最早一次——会话里同一编辑被重试过。
const seen = new Set();
const deduped = [];
for (const o of ops) {
  const key = `${o.rel}\u0000${o.old}\u0000${o.neu}`;
  if (seen.has(key)) continue;
  seen.add(key);
  deduped.push(o);
}

const lf = (s) => s.split("\r\n").join("\n");
const byFile = new Map();
for (const o of deduped) {
  if (!byFile.has(o.rel)) byFile.set(o.rel, []);
  byFile.get(o.rel).push(o);
}

let applied = 0;
let skipped = 0;
let missed = 0;
for (const [rel, list] of byFile) {
  if (!existsSync(rel)) continue;
  const raw = readFileSync(rel, "utf8");
  const crlf = raw.includes("\r\n");
  let cur = lf(raw);
  const changes = [];
  for (const o of list) {
    const oldLf = lf(o.old);
    const neuLf = lf(o.neu);
    // 已包含新内容 → 说明这条编辑已经在树里（历史提交里已生效）
    if (cur.includes(neuLf) && !cur.includes(oldLf)) {
      skipped++;
      continue;
    }
    if (!cur.includes(oldLf)) {
      missed++;
      continue;
    }
    cur = cur.replace(oldLf, () => neuLf);
    applied++;
    changes.push(`${new Date(o.ts).toISOString()}  +${neuLf.split("\n").length - oldLf.split("\n").length}`);
  }
  console.log(`\n### ${rel}   ops=${list.length}  本次应用 ${changes.length} 条`);
  for (const c of changes) console.log(`   ${c}`);
  if (APPLY && changes.length > 0) {
    writeFileSync(rel, crlf ? cur.split("\n").join("\r\n") : cur);
    console.log("   → 已写入");
  }
}
console.log(`\n合计：应用 ${applied}，已在树中 ${skipped}，未命中 ${missed}${APPLY ? "（已写入）" : "（干跑）"}`);
