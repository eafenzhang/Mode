import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { startOracleRuntime } from "./driver.mjs";

test("oracle 能起来、能吐事件、能干净退出", async () => {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-oracle-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-oracle-ws-"));
  await writeFile(join(cwd, "hello.txt"), "hello\n", "utf8");

  const session = await startOracleRuntime({ dataRoot, cwd });
  try {
    await session.send("读一下 hello.txt 然后回一句话作为回答：它写了什么？");
    const seen: string[] = [];
    for await (const event of session.events) {
      seen.push(event.type);
      if (event.type === "turn.completed" || event.type === "turn.failed") break;
    }
    // 这一条断言只钉「驱动真的收得到会话事件」，不钉模型输出——
    // 后续用例才拿它做行为对拍。
    assert.ok(seen.includes("turn.started"), `没收到 turn.started，实收：${seen.join(",")}`);
    assert.ok(
      seen.includes("turn.completed") || seen.includes("turn.failed"),
      `turn 没有终态，实收：${seen.join(",")}`,
    );
  } finally {
    await session.stop();
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
});
