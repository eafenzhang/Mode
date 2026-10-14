// 契约 A/B 的 oracle 断言（docs/specs/browser-import-helper-contract.md）。
// 只跑进程边界，不读旧实现源码（clean-room-policy §黑盒对拍）。
//
// 平台约束：旧产物是 Windows 专用 exe，且不入库（bundled-tools 被 gitignore）。
// 因此本文件在「旧产物不在场」时必须整条 skip —— verify job 跑在 ubuntu 上，
// 不能因为缺 Windows 产物把流水线弄红；而在 Windows 腿里由 CI 步骤先产出再跑。
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { test } from "node:test";
import {
  ORACLE_PATH,
  resolveRustHelperPath,
  runOracle,
  runRustHelper,
} from "../../../scripts/browser-import-helper-oracle.mjs";

const PROTOCOL = "ZCODE_BROWSER_IMPORT_HELPER";

const oracleSkip = existsSync(ORACLE_PATH)
  ? false
  : `旧产物不在场（${ORACLE_PATH}）；仅 Windows 且需先 prepare:browser-import-helper`;
const rustHelperPath = resolveRustHelperPath();
const rustSkip = rustHelperPath
  ? false
  : "rust helper 未构建（cargo build -p browser-import-helper）";

test("--version 输出 5 字段，协议名与协议版本为冻结常量", { skip: oracleSkip }, async () => {
  const result = await runOracle(["--version"]);
  assert.equal(result.exitCode, 0);
  const fields = result.stdout.trim().split("\t");
  assert.equal(fields.length, 5);
  assert.equal(fields[0], PROTOCOL);
  assert.equal(fields[1], "2");
  assert.match(fields[2], /^(x64|arm64)$/);
});

test("无参数、--help 与未知参数退出码为 2 且无 stdout", { skip: oracleSkip }, async () => {
  for (const args of [[], ["--help"], ["--nope"]]) {
    const result = await runOracle(args);
    assert.equal(result.exitCode, 2, `args=${args.join(" ")}`);
    assert.equal(result.stdout, "", `args=${args.join(" ")}`);
  }
});

// 契约 A 的黑盒对拍：字段 0/1/2 必须与旧产物逐字节一致；
// 字段 3/4（应用版本、git 修订）是构建期注入，允许不同。
test("Rust helper 的 --version 前 3 字段与旧产物一致", { skip: oracleSkip || rustSkip }, async () => {
  const oracle = await runOracle(["--version"]);
  const rust = await runRustHelper(["--version"]);
  assert.equal(rust.exitCode, 0);
  const frozen = (result) => result.stdout.trim().split("\t").slice(0, 3);
  assert.deepEqual(frozen(rust), frozen(oracle));
});

// broker 模式会触发 UAC 提权与服务通道，必须由人确认，默认不跑；
// 契约 B 的取数在实机验收清单里人工执行（见 port 计划 Task 7 Step 2）。
test(
  "broker 模式对畸形请求 fail-closed（不吐 OK 帧）",
  { skip: oracleSkip || process.env.MODE_BROWSER_IMPORT_ORACLE_UAC !== "1" },
  async () => {
    const result = await runOracle(
      ["--broker", "--parent-pid", String(process.pid)],
      "not-a-valid-frame\n",
    );
    assert.notEqual(result.stdout.trim().split("\t")[1], "OK");
  },
);
