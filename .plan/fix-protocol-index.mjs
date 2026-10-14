// 精确删除 mode-protocol/index.ts 里同步新加的悬空再导出块 + 一处重复导入行。
import { readFileSync, writeFileSync } from "node:fs";

const p = "packages/shared/src/mode-protocol/index.ts";
let src = readFileSync(p, "utf8");
const eol = src.includes("\r\n") ? "\r\n" : "\n";
let text = src.split("\r\n").join("\n");
const before = text.length;

// 1) 两段 `from "../bots.js"` 的再导出块（块内 8 个符号全部只存在于上游的 bots.ts）
const block = [
  "export {",
  "  botIdentitySchema,",
  "  botTopicInputMessageSchema,",
  "  botGroupInputSourceSchema,",
  "  type BotGroupInputSource,",
  '} from "../bots.js";',
  "export {",
  "  botTopicContextSchema,",
  "  botTopicMessageSchema,",
  "  type BotTopicHistoryBatch,",
  "  type BotTopicMessage,",
  '} from "../bots.js";',
].join("\n");
const blockCount = text.split(block).length - 1;
text = text.split(block).join("");

// 2) 重复的 modeWorkspaceRefSchema 导入（4 空格缩进那条；同块内另有一条 2 空格的在用）
const dup = "    modeWorkspaceRefSchema,\n    modeSyntheticUserMessageSourceSchema as legacyModeSyntheticUserMessageSourceSchema,\n";
// 只删重复的那一行本身
const dupLine = "    modeWorkspaceRefSchema,\n";
const dupCount = text.split(dupLine).length - 1;
if (dupCount === 1) text = text.split(dupLine).join("");

writeFileSync(p, eol === "\r\n" ? text.split("\n").join("\r\n") : text);
console.log(`再导出块命中 ${blockCount} 处；重复导入行命中 ${dupCount} 处；文件减少 ${before - text.length} 字节`);
console.log(`行数 ${text.split("\n").length}`);
