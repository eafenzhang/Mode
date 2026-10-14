// 契约 A/B 的 oracle 断言（docs/specs/browser-import-helper-contract.md）。
// 只跑进程边界，不读旧实现源码（clean-room-policy §黑盒对拍）。
import assert from "node:assert/strict";
import { test } from "node:test";
import { runOracle } from "../../../scripts/browser-import-helper-oracle.mjs";

const PROTOCOL = "ZCODE_BROWSER_IMPORT_HELPER";

test("--version 输出 5 字段，协议名与协议版本为冻结常量", async () => {
  const result = await runOracle(["--version"]);
  assert.equal(result.exitCode, 0);
  const fields = result.stdout.trim().split("\t");
  assert.equal(fields.length, 5);
  assert.equal(fields[0], PROTOCOL);
  assert.equal(fields[1], "2");
  assert.match(fields[2], /^(x64|arm64)$/);
});

test("无参数、--help 与未知参数退出码为 2 且无 stdout", async () => {
  for (const args of [[], ["--help"], ["--nope"]]) {
    const result = await runOracle(args);
    assert.equal(result.exitCode, 2, `args=${args.join(" ")}`);
    assert.equal(result.stdout, "", `args=${args.join(" ")}`);
  }
});

// broker 模式会触发 UAC 提权与服务通道，必须由人确认，默认不跑；
// 契约 B 的取数在实机验收清单里人工执行（见 port 计划 Task 7 Step 2）。
test(
  "broker 模式对畸形请求 fail-closed（不吐 OK 帧）",
  { skip: process.env.MODE_BROWSER_IMPORT_ORACLE_UAC !== "1" },
  async () => {
    const result = await runOracle(
      ["--broker", "--parent-pid", String(process.pid)],
      "not-a-valid-frame\n",
    );
    assert.notEqual(result.stdout.trim().split("\t")[1], "OK");
  },
);
