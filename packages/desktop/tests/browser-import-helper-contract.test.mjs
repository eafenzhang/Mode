// 契约 A/B 的黑盒断言（docs/specs/browser-import-helper-contract.md）。
// 只跑进程边界，不读旧实现源码（clean-room-policy §黑盒对拍）。
//
// 平台约束：旧产物是 Windows 专用 exe，且不入库（bundled-tools 被 gitignore）。
// 因此本文件在「旧产物不在场」时必须整条 skip —— verify job 跑在 ubuntu 上，
// 不能因为缺 Windows 产物把流水线弄红；而在 Windows 腿里由 CI 步骤先产出再跑。
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { test } from "node:test";
import {
  hasExplicitOracle,
  ORACLE_PATH,
  resolveRustHelperPath,
  runOracle,
  runRustHelper,
} from "../../../scripts/browser-import-helper-oracle.mjs";

// 宿主契约令牌（packages/desktop/src/main/windowsChromeAppBoundKey.ts 的
// HELPER_PROTOCOL / HELPER_VERSION_PROTOCOL）——这是我们要对齐的真值。
const HOST_WIRE_TOKEN = "MODE_BROWSER_IMPORT_V1";
const HOST_VERSION_TOKEN = "MODE_BROWSER_IMPORT_HELPER";
// 旧产物实测发的是 ZCODE_ 品牌（品牌改名漏了 .cs），与宿主断言不符 → 现役缺陷。
// 这里只把它当「旧产物的观察值」，不当作我们要对齐的契约。
const ORACLE_VERSION_TOKEN = "ZCODE_BROWSER_IMPORT_HELPER";

// 默认落点已被 Rust helper 占用（prepare:browser-import-helper 现指 Rust），
// 所以对拍旧 C# 产物必须显式给出路径，否则会用 Rust 产物冒充旧产物。
const oracleSkip = !hasExplicitOracle()
  ? "未指定 MODE_BROWSER_IMPORT_ORACLE_PATH（默认落点已是 Rust 产物）"
  : existsSync(ORACLE_PATH)
    ? false
    : `旧产物不在场（${ORACLE_PATH}）；先跑 prepare:browser-import-helper:oracle`;
const rustHelperPath = resolveRustHelperPath();
const rustSkip = rustHelperPath
  ? false
  : "rust helper 未构建（cargo build -p browser-import-helper）";

test("旧产物 --version 输出 5 字段（记录其品牌串）", { skip: oracleSkip }, async () => {
  const result = await runOracle(["--version"]);
  assert.equal(result.exitCode, 0);
  const fields = result.stdout.trim().split("\t");
  assert.equal(fields.length, 5);
  assert.equal(fields[0], ORACLE_VERSION_TOKEN);
  assert.equal(fields[1], "2");
  assert.match(fields[2], /^(x64|arm64)$/);
});

test("旧产物 无参数/未知参数 退出码为 2 且无 stdout", { skip: oracleSkip }, async () => {
  for (const args of [[], ["--help"], ["--nope"]]) {
    const result = await runOracle(args);
    assert.equal(result.exitCode, 2, `args=${args.join(" ")}`);
    assert.equal(result.stdout, "", `args=${args.join(" ")}`);
  }
});

test("Rust helper 的 --version 满足宿主契约", { skip: rustSkip }, async () => {
  const result = await runRustHelper(["--version"]);
  assert.equal(result.exitCode, 0);
  const fields = result.stdout.trim().split("\t");
  assert.equal(fields.length, 5);
  assert.equal(fields[0], HOST_VERSION_TOKEN);
  assert.equal(fields[1], "2");
  assert.match(fields[2], /^(x64|arm64)$/);
});

test("Rust helper 的畸形 broker 请求回宿主线令牌的 ERR 帧", { skip: rustSkip }, async () => {
  const result = await runRustHelper(["--broker", "--parent-pid", "1"], "garbage\n");
  assert.equal(result.stdout.trim(), `${HOST_WIRE_TOKEN}\tERR\tvalidation_failed`);
});

// 对拍：只比「契约规定必须一致」的字段。第 1 字段是品牌串，旧产物有缺陷，
// 已由上面两条分别按各自口径断言；第 4/5 字段是构建期注入。
test("Rust 与旧产物的协议版本和架构字段一致", { skip: oracleSkip || rustSkip }, async () => {
  const oracle = await runOracle(["--version"]);
  const rust = await runRustHelper(["--version"]);
  const comparable = (result) => result.stdout.trim().split("\t").slice(1, 3);
  assert.deepEqual(comparable(rust), comparable(oracle));
});
