// 取出定义 CONVERSATION_MIRROR_DIR_NAME 的那次写入内容（只读）。
import { DatabaseSync } from "node:sqlite";
const db = new DatabaseSync(process.env.HOME + "/.mode/cli/db/db.sqlite", { readOnly: true });
const rows = db.prepare(`SELECT time_created, data FROM part WHERE data LIKE '%"tool":"Write"%'`).all();
for (const r of rows) {
  let obj;
  try {
    obj = JSON.parse(r.data);
  } catch {
    continue;
  }
  const input = obj?.state?.input ?? obj?.input;
  const content = input?.content;
  if (typeof content !== "string" || !content.includes("CONVERSATION_MIRROR_DIR_NAME")) continue;
  const ts = new Date(Number(r.time_created)).toISOString();
  const idx = content.indexOf("CONVERSATION_MIRROR_DIR_NAME");
  console.log(`\n${ts}  ${input.file_path}  (${content.length} bytes)`);
  console.log("…" + content.slice(Math.max(0, idx - 400), idx + 200) + "…");
}
