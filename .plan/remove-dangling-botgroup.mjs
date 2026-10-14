// 移除同步新加的悬空引用（这些符号只存在于上游的 bots.ts，我们这份没有）。
import { readFileSync, writeFileSync } from "node:fs";

// 1) mode-protocol/index.ts：整块删除 `export { … } from "../bots.js";`（块内 8 个符号全都不存在于我们的 bots.ts）
{
  const p = "packages/shared/src/mode-protocol/index.ts";
  const src = readFileSync(p, "utf8");
  const lines = src.split("\n");
  const out = [];
  let inBlock = false;
  let removed = 0;
  for (const line of lines) {
    if (!inBlock && /^export \{$/.test(line)) {
      inBlock = "pending";
      out.push({ line, idx: out.length });
      continue;
    }
    if (inBlock === "pending" && /^\} from "\.\.\/bots\.js";$/.test(line)) {
      // 整个块只含 bots.js 的再导出 → 丢弃已暂存的行与本行
      while (out.length && out[out.length - 1].idx === undefined) out.pop();
      out.pop();
      removed++;
      inBlock = false;
      continue;
    }
    if (inBlock === "pending") {
      out.push({ line, idx: undefined });
      continue;
    }
    out.push(line);
  }
  const text = out.map((x) => (typeof x === "string" ? x : x.line)).join("\n");
  writeFileSync(p, text);
  console.log(`${p}: 删除 ${removed} 个 from "../bots.js" 的再导出块`);
}

// 2) 三个 v4 文件：删掉 botGroupInputSourceSchema 的 import 与 botGroupSource 字段
const v4 = [
  ["packages/shared/src/mode-protocol-v4/rows.ts", /^import \{ botGroupInputSourceSchema \} from "\.\.\/bots\.js";\n/, /^  botGroupSource: botGroupInputSourceSchema\.optional\(\),\n/m],
  ["packages/shared/src/mode-protocol-v4/input-intent.ts", /^import \{ botGroupInputSourceSchema \} from "\.\.\/bots\.js";\n/, /^    botGroupSource: botGroupInputSourceSchema\.optional\(\),\n/m],
];
for (const [p, importRe, fieldRe] of v4) {
  let src = readFileSync(p, "utf8");
  const before = src.length;
  src = src.replace(importRe, "").replace(fieldRe, "");
  writeFileSync(p, src);
  console.log(`${p}: ${before - src.length} 字节被移除`);
}

// 3) command.ts：import 行上只摘掉 botGroupInputSourceSchema
{
  const p = "packages/shared/src/mode-protocol-v4/command.ts";
  let src = readFileSync(p, "utf8");
  const before = src.length;
  src = src
    .replace(
      /^import \{ botGroupInputSourceSchema, modeAutomationBotDeliveryTargetSchema \} from "\.\.\/bots\.js";$/m,
      'import { modeAutomationBotDeliveryTargetSchema } from "../bots.js";',
    )
    .replace(/^      botGroupSource: botGroupInputSourceSchema\.optional\(\),\n/m, "");
  writeFileSync(p, src);
  console.log(`${p}: ${before - src.length} 字节被移除`);
}
