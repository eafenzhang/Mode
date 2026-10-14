// 按 tsc 报的 (行,列) 给 re-export 里的类型符号加 `type ` 前缀（TS1205）。
import { readFileSync, writeFileSync } from "node:fs";

const sites = [
  ["packages/services/src/index.ts", [299, 303, 308, 308]],
  ["packages/shared/src/index.ts", [313, 314, 314, 317, 318]],
];
const cols = {
  "packages/services/src/index.ts": { 299: [10], 303: [10], 308: [10, 38] },
  "packages/shared/src/index.ts": { 313: [125], 314: [10, 18], 317: [10], 318: [10] },
};

for (const [file] of sites) {
  const raw = readFileSync(file, "utf8");
  const eol = raw.includes("\r\n") ? "\r\n" : "\n";
  const lines = raw.split("\r\n").join("\n").split("\n");
  for (const [lineNo, positions] of Object.entries(cols[file])) {
    const idx = Number(lineNo) - 1;
    let line = lines[idx];
    // 从右到左插入，避免列号漂移
    for (const col of [...positions].sort((a, b) => b - a)) {
      // col 是 1-based，指向标识符首字符
      const start = col - 1;
      let s = start;
      while (s > 0 && /[A-Za-z0-9_]/.test(line[s - 1])) s--;
      const before = line.slice(0, s);
      if (/type\s+$/.test(before)) continue;
      line = `${line.slice(0, s)}type ${line.slice(s)}`;
    }
    lines[idx] = line;
    console.log(`${file}:${lineNo} → ${line.trim()}`);
  }
  writeFileSync(file, eol === "\r\n" ? lines.join("\r\n") : lines.join("\n"));
}
