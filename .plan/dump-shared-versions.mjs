// 取出 shared/conversation-mirror.ts 在覆盖前的两个版本，便于合并（只读）。
import { DatabaseSync } from "node:sqlite";
import { writeFileSync } from "node:fs";
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
  const fp = (input?.file_path ?? "").split("\\").join("/");
  if (!fp.endsWith("packages/shared/src/conversation-mirror.ts")) continue;
  const ts = new Date(Number(r.time_created)).toISOString().replace(/[:.]/g, "-");
  const out = `D:/ZCodium/.plan/mirror-shared-${ts}.ts`;
  writeFileSync(out, input.content);
  console.log(`${ts}  ${input.content.length} bytes  → ${out}`);
}
