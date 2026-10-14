import { execSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
const run = (cmd) => { try { return execSync(cmd, { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 }); } catch (e) { return String(e.stdout ?? "") + String(e.stderr ?? ""); } };
const logFile = process.argv[2];
const log = logFile ? readFileSync(logFile, "utf8") : run(process.argv[3]);
const idx = readFileSync("packages/shared/src/index.ts", "utf8");
const nodeTs = readFileSync("packages/shared/src/node.ts", "utf8");
const missing = new Map();
for (const m of log.matchAll(/'"@mode\/shared(\/node)?"' has no exported member (?:named )?'([^']+)'/g)) {
  missing.set(m[2], m[1] ? "node" : "index");
}
console.log("missing:", missing.size, [...missing.keys()].slice(0, 12).join(", "));
const lines = { index: [], node: [] };
for (const [sym, barrel] of missing) {
  const g = run(`git grep -nE "^export (type|interface|const|function|class|enum|declare) ${sym}\\b" -- packages/shared/src`);
  const first = g.split("\n").find((l) => l.trim());
  if (!first) { console.log("  ?? not in shared/src:", sym); continue; }
  const file = first.split(":")[0];
  const rel = file.replace(/^packages\/shared\/src\//, "").replace(/\.ts$/, ".js");
  const decl = first.slice(first.indexOf(":", first.indexOf(":") + 1) + 1).trim();
  const isType = /^export (type|interface)\b/.test(decl);
  const line = `export ${isType ? "type " : ""}{ ${sym} } from './${rel}';`;
  const target = barrel === "node" ? nodeTs : idx;
  const re = new RegExp(`[{,\\s]${sym}[,\\s}]`);
  if (re.test(target)) continue;
  lines[barrel].push(line);
}
if (lines.index.length) writeFileSync("packages/shared/src/index.ts", idx.replace(/\n?$/, "\n") + lines.index.join("\n") + "\n");
if (lines.node.length) writeFileSync("packages/shared/src/node.ts", nodeTs.replace(/\n?$/, "\n") + lines.node.join("\n") + "\n");
console.log("added index:", lines.index.length, "node:", lines.node.length);
