/**
 * IM bot SDK 懒加载守卫（路线图 M4a 可选项：IM bot SDK 可选化）。
 *
 * 裁决（2026-10-23）：larksuite / wecom SDK 按通道动态 import，未启用通道时
 * Host 启动不加载它们。本用例是这条口径的**源码级回归闸**：
 * - 扫描 `src/bots/**` 全部 TS 文件，禁止对两个 SDK 的**值 import**
 *   （`import { ... } from "@wecom/aibot-node-sdk"`）；`import type` 编译期擦除，允许；
 * - 动态 `import("...")` 只允许出现在两个已知懒加载点：
 *   `wecomSdk.ts`（wecom 唯一收口）与 `feishuProvider.ts`（启动长连接前）。
 *
 * 为什么用源码扫描而不是运行时断言：「未加载」在进程内很难可观测
 * （模块缓存是黑盒）；而静态 import 一旦回潮，启动路径就会重新拉起 25 MB 的
 * larksuite——源码扫描能在这条线被踩到之前就红。
 */
import assert from "node:assert/strict";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const BOTS_SRC = fileURLToPath(new URL("../src/bots", import.meta.url));

/** 允许出现动态 `import("<sdk>")` 的文件（相对 bots 根的 POSIX 路径）。 */
const DYNAMIC_IMPORT_ALLOWLIST = new Set(["wecomSdk.ts", "providers/feishuProvider.ts"]);

function collectTsFiles(dir: string, relative = ""): string[] {
  const result: string[] = [];
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    const rel = relative ? `${relative}/${entry}` : entry;
    if (statSync(full).isDirectory()) {
      result.push(...collectTsFiles(full, rel));
    } else if (entry.endsWith(".ts")) {
      result.push(rel);
    }
  }
  return result;
}

/** 去掉行注释与块注释：守卫只看代码，不误伤注释里提到的 import 示例。 */
function stripComments(source: string): string {
  return source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/\/\/.*$/gm, "");
}

test("IM bot SDK 不得被静态值 import（只允许 type import 与白名单动态 import）", () => {
  const offenders: string[] = [];
  for (const relative of collectTsFiles(BOTS_SRC)) {
    const code = stripComments(readFileSync(join(BOTS_SRC, relative), "utf8"));
    // 值 import：`import {` 或 `import 标识符`（无 type 修饰）+ SDK 说明符。
    // `import type {` 与 `import type 标识符` 在下一步被排除。
    const valueImport = /(^|\n)\s*import\s+(?!type\b)[^;\n]*?from\s*["'](@wecom\/aibot-node-sdk|@larksuiteoapi\/node-sdk)["']/g;
    if (valueImport.test(code)) {
      offenders.push(`${relative}: 静态值 import`);
    }
    const dynamicImport = /import\s*\(\s*["'](@wecom\/aibot-node-sdk|@larksuiteoapi\/node-sdk)["']\s*\)/g;
    if (dynamicImport.test(code) && !DYNAMIC_IMPORT_ALLOWLIST.has(relative)) {
      offenders.push(`${relative}: 动态 import 不在白名单（懒加载点应经 wecomSdk.ts / feishuProvider.ts）`);
    }
  }
  assert.deepEqual(
    offenders,
    [],
    `IM bot SDK 懒加载守卫失败：\n${offenders.join("\n")}\n` +
      "值 import 会让 Host 启动就加载 SDK；新增懒加载点请更新本用例的白名单并说明理由。",
  );
});

test("wecom 懒加载器缓存同一 promise（并发通道启动只解析一次）", async () => {
  const { loadWeComSdk } = await import("../src/bots/wecomSdk.js");
  const first = loadWeComSdk();
  const second = loadWeComSdk();
  assert.equal(first, second, "两次调用必须返回同一 promise 引用");
  const sdk = await first;
  assert.equal(typeof sdk.WSClient, "function", "SDK 模块应导出 WSClient");
  assert.equal(typeof sdk.generateReqId, "function", "SDK 模块应导出 generateReqId");
});
