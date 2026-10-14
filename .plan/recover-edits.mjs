// 从 mode 会话数据库里抽出被 d22c60f3 覆盖掉的那批编辑（只读）。
import { DatabaseSync } from "node:sqlite";

const REL = [
  "packages/services/src/paths.ts",
  "packages/services/src/node.ts",
  "packages/services/src/index.ts",
  "packages/shared/src/channels.ts",
  "packages/shared/src/remote-workspace-identity.ts",
  "packages/ui/src/BotsDialog.tsx",
  "packages/ui/src/BotsDialog/ProviderSettingsCard.tsx",
  "packages/ui/src/assets/provider-icons/model-provider-logo-sources.json",
];

const CLOBBER = Date.parse("2026-10-13T18:42:36Z");
const onlyBefore = process.argv.includes("--before");
const wantFile = process.argv.find((a) => a.startsWith("--file="))?.slice("--file=".length);
const dump = process.argv.includes("--dump");

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
  if (typeof fp !== "string") continue;
  const norm = fp.split("\\").join("/");
  const rel = REL.find((x) => norm.endsWith(x));
  if (!rel) continue;
  if (wantFile && rel !== wantFile) continue;
  if (onlyBefore && Number(r.time_created) >= CLOBBER) continue;
  const meta = obj?.state?.metadata?.display;
  out.push({
    ts: Number(r.time_created),
    session: r.session_id,
    rel,
    tool: obj.tool,
    old: input.old_string,
    neu: input.new_string ?? input.content,
    add: meta?.additions,
    del: meta?.deletions,
  });
}
out.sort((a, b) => a.ts - b.ts);

if (!dump) {
  console.log(`共 ${out.length} 条编辑\n`);
  for (const e of out) {
    console.log(
      `${new Date(e.ts).toISOString()}  ${e.ts < CLOBBER ? "覆盖前" : "覆盖后"}  ${e.tool}  ${e.rel}  +${e.add ?? "?"}/-${e.del ?? "?"}  ${e.session}`,
    );
  }
} else {
  for (const e of out) {
    console.log(`\n${"=".repeat(80)}`);
    console.log(`${new Date(e.ts).toISOString()}  ${e.tool}  ${e.rel}  +${e.add}/-${e.del}  ${e.session}`);
    if (e.old !== undefined) console.log(`--- OLD ---\n${e.old}`);
    console.log(`--- NEW ---\n${e.neu}`);
  }
}
