import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";

import { assertDataRootIsolated, cliLayoutSentinelPath, startOracleRuntime } from "./driver.mjs";

test("数据根隔离：指向用户真实数据根（含多代旧根）时直接失败，不静默跑", () => {
  for (const name of [".mode", ".zcodium", ".zcode"]) {
    assert.throws(() => assertDataRootIsolated(join(homedir(), name)), /用户真实数据根/);
  }
});

test("隔离根落在 mkdtemp 下：归属文件与 CLI 布局都在隔离根里、模型流量全部打到 mock provider", async () => {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-oracle-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-oracle-ws-"));
  await writeFile(join(cwd, "hello.txt"), "hello\n", "utf8");

  const session = await startOracleRuntime({ dataRoot, cwd });
  try {
    await session.send("读一下 hello.txt 然后回一句话作为回答：它写了什么？");
    for await (const event of session.events) {
      if (event.type === "turn.completed" || event.type === "turn.failed") break;
    }
    // 两条隔离事实都要在（driver 启动时已断言过一次；这里再钉一次，
    // 防止将来把那条断言删掉还全绿）。**CLI 布局**这条是真正的哨兵：
    // 只设 MODE_DATA_BASE_DIR 时归属文件照样落在隔离根，但 CLI 布局会漏到用户真实
    // ~/.mode/cli —— 那种情况下归属文件断言全绿（spec §1.4 的实测事实）。
    assert.ok(
      existsSync(join(dataRoot, ".mode", ".mode-root.json")),
      "隔离根里没有 .mode/.mode-root.json：数据根隔离没有生效",
    );
    assert.ok(
      existsSync(cliLayoutSentinelPath(dataRoot)),
      `隔离根里没有 .mode/cli/log：CLI 布局漏到用户真实数据根了，本次运行正在污染生产数据`,
    );
    // Step 4b 的判据：模型流量确实打到本地 mock 端点，而不是真实模型端点。
    assert.ok(
      session.mockRequestPaths.some((path) => path === "/v1/messages"),
      `mock provider 没有收到 /v1/messages，实收：${session.mockRequestPaths.join(",")}`,
    );
    // 解析失败的行不静默丢：对拍用例的输入必须是干净的。
    assert.deepEqual(session.invalidLines, [], "stdout 有无法解析的行");
  } finally {
    await session.stop();
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
});
